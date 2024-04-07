mod triangulate;

use std::collections::VecDeque;
use std::time::Instant;
use std::{cmp, io};

use pawawwewism::{promise, Promise, PromiseHandle, Worker};
use providence_io::data::{FaceData, PersistentId, TrackingMessage};
use providence_io::net::Publisher;
use tracing::debug_span;
use triangulate::{Side, Triangulator};
use zaru::detection::Detector;
use zaru::face::detection::ShortRangeNetwork;
use zaru::face::landmark::mediapipe::{self, FaceMeshV2, LandmarkResultV2};
use zaru::filter::one_euro::OneEuroFilter;
use zaru::filter::{TimeBasedFilter, TimedFilterAdapter};
use zaru::image::Image;
use zaru::landmark::{Estimator, LandmarkFilter, LandmarkTracker};
use zaru::linalg::{vec3, Quat};
use zaru::num::TotalF32;
use zaru::procrustes::ProcrustesAnalyzer;
use zaru::rect::RotatedRect;
use zaru::video::webcam::{ParamPreference, Webcam, WebcamOptions};

const TIMESTAMP_OFFSET: u32 = u32::MAX - 10_000_000; // 10 seconds before overflow

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

    let reference_time = Instant::now();
    let mut publisher = Publisher::spawn()?;
    let mut message_queue = VecDeque::new();
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
        let timestamp = Instant::now().duration_since(reference_time).as_micros()
            + u128::from(TIMESTAMP_OFFSET);

        let (landmarks, landmarks_handle) = promise();
        let (message, message_handle) = promise();
        face_tracker.send(FaceTrackParams { image, landmarks });
        assembler.send(AssemblerParams {
            landmarks: landmarks_handle,
            message,
        });
        message_queue.push_back(message_handle);

        if let Some(handle) = message_queue.front() {
            if !handle.will_block() {
                let mut message = match message_queue.pop_front().unwrap().block() {
                    Ok(msg) => msg,
                    Err(_) => {
                        // If this promise was dropped, no face was detected.
                        TrackingMessage {
                            timestamp: 0,
                            faces: Vec::new(),
                        }
                    }
                };

                message.timestamp = timestamp as u32;
                publisher.publish(message);
            }
        }
    }
}

struct AssemblerParams {
    landmarks: PromiseHandle<(LandmarkResultV2, Image)>,
    message: Promise<TrackingMessage>,
}

fn assembler() -> Result<Worker<AssemblerParams>, io::Error> {
    let mut procrustes_analyzer = ProcrustesAnalyzer::new(mediapipe::reference_positions());
    let mut tri = Triangulator::new();

    Worker::builder()
        .name("assembler")
        .spawn(move |AssemblerParams { landmarks, message }| {
            let Ok((mut face_landmark, image)) = landmarks.block() else {
                return;
            };

            let procrustes_result = debug_span!("procrustes").in_scope(|| {
                procrustes_analyzer.analyze(face_landmark.mesh_landmarks().map(|lm| {
                    // Flip Y to bring us to canonical 3D coordinates (where Y points up).
                    // Only rotation matters, so we don't have to correct for the added
                    // translation.
                    vec3(lm.x(), -lm.y(), lm.z())
                }))
            });

            let [x, y, z] = procrustes_result.rotation().to_rotation_xyz();
            // Invert the angles so that the reported head rotation matches what looking in a mirror
            // is like.
            let head_rotation = Quat::from_rotation_xyz(-x, y, -z);
            let head_rotation_inv = head_rotation.conjugate();

            let (left_eye, right_eye) = debug_span!("triangulate").in_scope(|| {
                (
                    tri.triangulate_eye(&face_landmark, &image, Side::Left, head_rotation_inv),
                    tri.triangulate_eye(&face_landmark, &image, Side::Right, head_rotation_inv),
                )
            });

            // Mirror the whole image, so that the eyes match what the user does.
            let (right_eye, left_eye) = (left_eye.flip_horizontal(), right_eye.flip_horizontal());

            // Map all landmarks into range 0..=1 for computing the head position
            let max = cmp::max(image.width(), image.height()) as f32;
            face_landmark.landmarks_mut().map_positions(|p| p / max);
            let avg = face_landmark.landmarks().average_position();

            message.fulfill(TrackingMessage {
                timestamp: 0, // filled in later
                faces: vec![FaceData {
                    ephemeral_id: 0,
                    persistent_id: PersistentId::Unavailable,
                    head_position: [1.0 - avg.x, avg.y],
                    head_rotation: [
                        head_rotation.i,
                        head_rotation.j,
                        head_rotation.k,
                        head_rotation.w,
                    ],
                    left_eye: left_eye.into_message(),
                    right_eye: right_eye.into_message(),
                }],
            });
        })
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
                    let rect = target.bounding_rect().move_by(view_rect.top_left());
                    let rect = RotatedRect::new(rect, target.angle());
                    tracing::trace!("start tracking face at {:?}", rect);
                    tracker.set_roi(rect);
                }
            }

            if let Some(res) = tracker.track(&image) {
                landmarks.fulfill((res.estimate().clone(), image));
            }
        })
}

type Filt = OneEuroFilter;
fn filter() -> TimedFilterAdapter<Filt> {
    Filt::new(0.0001, 0.3).real_time()
}
