mod triangulate;

use std::collections::VecDeque;
use std::{cmp, io};

use nalgebra::UnitQuaternion;
use pawawwewism::{promise, Promise, PromiseHandle, Worker};
use providence_io::data::TrackingMessage;
use providence_io::net::Publisher;
use triangulate::{Eye, Triangulator};
use zaru::detection::Detector;
use zaru::face::detection::ShortRangeNetwork;
use zaru::face::landmark::mediapipe::{self, FaceMeshV2, LandmarkResultV2};
use zaru::filter::one_euro::OneEuroFilter;
use zaru::filter::{TimeBasedFilter, TimedFilterAdapter};
use zaru::image::Image;
use zaru::landmark::{Estimator, LandmarkFilter, LandmarkTracker};
use zaru::num::TotalF32;
use zaru::procrustes::ProcrustesAnalyzer;
use zaru::rect::RotatedRect;
use zaru::timer::{FpsCounter, Timer};
use zaru::video::webcam::{ParamPreference, Webcam, WebcamOptions};

#[zaru::main]
fn main() -> anyhow::Result<()> {
    let mut face_tracker = face_track_worker()?;
    let mut assembler = assembler()?;

    let mut webcam = Webcam::open(
        WebcamOptions::default()
            .fps(30)
            .prefer(ParamPreference::Resolution),
    )?;
    webcam.read()?;

    let mut publisher = Publisher::spawn()?;
    let mut message_queue = VecDeque::new();
    let mut fps = FpsCounter::new("webcam");
    loop {
        // To avoid wasting CPU, we only perform processing when there is a client connected.
        // Ideally we'd also clear the face tracking state, but that's kinda difficult to do.
        if !publisher.has_connection() {
            // Make sure to drop old messages so that we don't sent anything outdated to new clients.
            message_queue.clear();
            publisher.clear();
            publisher.block_until_connected();
            // FIXME: blocking here prevents us from detecting that the webcam disappeared until a client connects!
            // it'd be better to do this `async`.

            // Drain all pending webcam frames to ensure we resume with more recent frames.
            webcam.flush()?;
        }

        // NB: the non-flipped webcam image is "the wrong way around" - but flipping the whole image
        // is *very* expensive for some reason, so we only flip the final result.
        let image = webcam.read()?;

        let (landmarks, landmarks_handle) = promise();
        let (message, message_handle) = promise();
        face_tracker.send(FaceTrackParams { image, landmarks });
        assembler.send(AssemblerParams {
            landmarks: landmarks_handle,
            message,
        });
        message_queue.push_back(message_handle);

        fps.tick_with(webcam.timers());

        if let Some(handle) = message_queue.front() {
            if !handle.will_block() {
                if let Ok(msg) = message_queue.pop_front().unwrap().block() {
                    publisher.publish(msg);
                }
            }
        }
    }
}

struct AssemblerParams {
    landmarks: PromiseHandle<(LandmarkResultV2, Image)>,
    message: Promise<TrackingMessage>,
}

fn assembler() -> Result<Worker<AssemblerParams>, io::Error> {
    let mut fps = FpsCounter::new("assembler");
    let t_procrustes = Timer::new("procrustes");
    let t_triangulate = Timer::new("triangulate");

    let mut procrustes_analyzer = ProcrustesAnalyzer::new(mediapipe::reference_positions());
    let mut tri = Triangulator::new();

    Worker::builder().name("assembler").spawn(
        move |AssemblerParams {
                  landmarks,
                  message,
              }| {
            let Ok((mut face_landmark, image)) = landmarks.block() else { return };

            let procrustes_result = t_procrustes.time(|| {
                procrustes_analyzer.analyze(face_landmark.mesh_landmarks().map(
                    |lm| {
                        // Flip Y to bring us to canonical 3D coordinates (where Y points up).
                        // Only rotation matters, so we don't have to correct for the added
                        // translation.
                        (lm.x(), -lm.y(), lm.z())
                    },
                ))
            });

            let (r, p, y) = procrustes_result.rotation().euler_angles();
            // Invert the angles so that the reported head rotation matches what looking in a mirror
            // is like.
            let quat = UnitQuaternion::from_euler_angles(-r, p, -y);
            let head_rotation = [quat.i, quat.j, quat.k, quat.w];

            let guard = t_triangulate.start();
            let Ok(left_eye) = tri.triangulate_eye(&face_landmark, &image, Eye::Left) else { return };
            let Ok(right_eye) = tri.triangulate_eye(&face_landmark, &image, Eye::Right) else { return };
            drop(guard);

            // Mirror the whole image, so that the eyes match what the user does.
            let (right_eye, left_eye) = (left_eye.flip_horizontal(), right_eye.flip_horizontal());

            // Map all landmarks into range 0..=1 for computing the head position
            let max = cmp::max(image.width(), image.height()) as f32;
            face_landmark.landmarks_mut()
                .map_positions(|[x, y, z]| [x / max, y / max, z / max]);
            let [x, y, _] = face_landmark.landmarks().average_position();
            let head_position = [1.0 - x, y];

            message.fulfill(TrackingMessage {
                head_position,
                head_rotation,
                left_eye: left_eye.into_message(),
                right_eye: right_eye.into_message(),
            });

            fps.tick_with([&t_procrustes, &t_triangulate]);
        },
    )
}

struct FaceTrackParams {
    image: Image,
    landmarks: Promise<(LandmarkResultV2, Image)>,
}

/// The face track worker is sent the decoded webcam image and does the following:
///
/// - Detect faces (if none are currently tracked)
/// - Compute facial landmarks and track their positions across frames
/// - Copy the eye's regions from the image and send them to the eye tracking workers
fn face_track_worker() -> Result<Worker<FaceTrackParams>, io::Error> {
    let mut fps = FpsCounter::new("tracker");
    let t_total = Timer::new("total");

    let mut detector = Detector::new(ShortRangeNetwork);
    let mut estimator = Estimator::new(FaceMeshV2);
    estimator.set_filter(LandmarkFilter::new(
        filter(),
        LandmarkResultV2::NUM_LANDMARKS,
    ));
    let mut tracker = LandmarkTracker::new(estimator);
    let input_ratio = detector.input_resolution().aspect_ratio().unwrap();

    Worker::builder()
        .name("face tracker")
        .spawn(move |FaceTrackParams { image, landmarks }| {
            let guard = t_total.start();

            if tracker.roi().is_none() {
                // Zoom into the camera image and perform detection there. This makes outer
                // edges of the camera view unusable, but significantly improves the tracking
                // distance.
                let view_rect = image.resolution().fit_aspect_ratio(input_ratio);
                let view = image.view(view_rect);
                let detections = detector.detect(&view);

                if let Some(target) = detections
                    .iter()
                    .max_by_key(|det| TotalF32(det.confidence()))
                {
                    let rect = target.bounding_rect().move_by(view_rect.x(), view_rect.y());
                    let rect = RotatedRect::new(rect, target.angle());
                    log::trace!("start tracking face at {:?}", rect);
                    tracker.set_roi(rect);
                }
            }

            if let Some(res) = tracker.track(&image) {
                landmarks.fulfill((res.estimate().clone(), image));
            }

            drop(guard);
            fps.tick_with(
                [&t_total]
                    .into_iter()
                    .chain(detector.timers())
                    .chain(tracker.timers()),
            );
        })
}

type Filt = OneEuroFilter;
fn filter() -> TimedFilterAdapter<Filt> {
    Filt::new(0.0001, 0.3).real_time()
}
