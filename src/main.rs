mod triangulate;

use std::collections::VecDeque;
use std::time::Instant;
use std::{cmp, io};

use pawawwewism::{promise, Promise, PromiseHandle, Worker};
use providence_io::data::{FaceData, PersistentId, TrackingMessage};
use providence_io::net::Publisher;
use triangulate::{Side, Triangulator};
use zaru::detection::{Detection, Detector};
use zaru::face::detection::ShortRangeNetwork;
use zaru::face::landmark::mediapipe::{self, FaceMeshV2, LandmarkResultV2};
use zaru::filter::one_euro::OneEuroFilter;
use zaru::filter::{TimeBasedFilter, TimedFilterAdapter};
use zaru::image::histogram::Histogram;
use zaru::image::lut::Lut;
use zaru::image::{rect::RotatedRect, Image};
use zaru::landmark::{Estimator, LandmarkFilter, LandmarkTracker};
use zaru::linalg::{vec3, Quat};
use zaru::num::TotalF32;
use zaru::procrustes::ProcrustesAnalyzer;
use zaru::profile;
use zaru::video::webcam::{ParamPreference, Webcam, WebcamOptions};

const TIMESTAMP_OFFSET: u32 = u32::MAX - 10_000_000; // 10 seconds before overflow

const ENABLE_POSTPROC: bool = false;

fn webcam_opts() -> WebcamOptions {
    WebcamOptions::default()
        .fps(30)
        .prefer(ParamPreference::Resolution)
}

#[zaru::main]
fn main() -> anyhow::Result<()> {
    let mut face_tracker = face_track_worker()?;
    let mut assembler = assembler()?;

    let mut webcam = Webcam::open(webcam_opts())?;
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

            // Close the webcam device. We reopen it when a client connects. This allows the webcam
            // to be idle or even replugged while the tracker is idle, and allows the tracker to
            // survive system suspend.
            drop(webcam);
            publisher.block_until_connected();

            webcam = Webcam::open(webcam_opts())?;
        }

        // NB: the non-flipped webcam image is "the wrong way around" - we flip the data/sprites in
        // the assembler.
        let image = webcam.read()?;
        let timestamp = Instant::now().duration_since(reference_time).as_micros()
            + u128::from(TIMESTAMP_OFFSET);

        let (output, landmarks_handle) = promise();
        let (message, message_handle) = promise();
        face_tracker.send(FaceTrackParams { image, output });
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
    landmarks: PromiseHandle<(TrackerOutput, Image)>,
    message: Promise<TrackingMessage>,
}

fn assembler() -> Result<Worker<AssemblerParams>, io::Error> {
    let mut procrustes_analyzer = ProcrustesAnalyzer::new(mediapipe::reference_positions());
    let mut tri = Triangulator::new();

    Worker::builder()
        .name("assembler")
        .spawn(move |AssemblerParams { landmarks, message }| {
            let Ok((output, image)) = landmarks.block() else {
                return;
            };

            match output {
                TrackerOutput::Landmarks(mut face_landmark) => {
                    let procrustes_result = profile::scope("procrustes", || {
                        procrustes_analyzer.analyze(face_landmark.mesh_landmarks().map(|lm| {
                            // Flip Y to bring us to canonical 3D coordinates (where Y points up).
                            // Only rotation matters, so we don't have to correct for the added
                            // translation.
                            vec3(lm.x, -lm.y, lm.z)
                        }))
                    });

                    let [x, y, z] = procrustes_result.rotation().to_rotation_xyz();
                    // Invert the angles so that the reported head rotation matches what looking in a mirror
                    // is like.
                    let head_rotation = Quat::from_rotation_xyz(-x, y, -z);
                    let head_rotation_inv = head_rotation.conjugate();

                    let (left_eye, right_eye) = profile::scope("triangulate", || {
                        (
                            tri.triangulate_eye(
                                &face_landmark,
                                &image,
                                Side::Left,
                                head_rotation_inv,
                            ),
                            tri.triangulate_eye(
                                &face_landmark,
                                &image,
                                Side::Right,
                                head_rotation_inv,
                            ),
                        )
                    });

                    // Mirror the whole image, so that the eyes match what the user does.
                    let (mut right_eye, mut left_eye) =
                        (left_eye.flip_horizontal(), right_eye.flip_horizontal());
                    postprocess_eye_sprites(&mut left_eye.texture, &mut right_eye.texture);

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
                            left_eye: Some(left_eye.into_message()),
                            right_eye: Some(right_eye.into_message()),
                        }],
                    });
                }
                TrackerOutput::Detection(det) => {
                    // Map all landmarks into range 0..=1 for computing the head position
                    let max = cmp::max(image.width(), image.height()) as f32;
                    let pos = det.bounding_rect().center() / max;

                    let head_rotation = Quat::from_rotation_z(det.angle());
                    message.fulfill(TrackingMessage {
                        timestamp: 0, // filled in later
                        faces: vec![FaceData {
                            ephemeral_id: 0,
                            persistent_id: PersistentId::Unavailable,
                            head_position: [1.0 - pos.x, pos.y],
                            head_rotation: [
                                head_rotation.i,
                                head_rotation.j,
                                head_rotation.k,
                                head_rotation.w,
                            ],
                            left_eye: None,
                            right_eye: None,
                        }],
                    })
                }
            }
        })
}

fn postprocess_eye_sprites(left: &mut Image, right: &mut Image) {
    if !ENABLE_POSTPROC {
        return;
    }
    profile::scope("postprocess", || {
        postprocess_eye_sprite(left);
        postprocess_eye_sprite(right);
    });
}

fn postprocess_eye_sprite(image: &mut Image) {
    let Some(hist) = Histogram::compute(&*image) else {
        return;
    };

    // From: "Automatic gamma correction based on average of brightness" (Babakhani et al., 2015)
    let avg = hist.average() / hist.bucket_count() as f32;
    let gamma = (-0.3) / avg.log10();

    Lut::from_gamma(gamma).apply(image);
}

struct FaceTrackParams {
    image: Image,
    output: Promise<(TrackerOutput, Image)>,
}

/// Per-face face tracker output.
///
/// The face tracker can be in 3 different "modes":
/// - normal mode: the face is fully visible and landmarks are available.
/// - degraded mode: the face is too obscured to compute landmarks on, but is still detected in the
///   image.
/// - "none" mode: no face is in view at all; if the tracker is in this mode the `Promise` will
///   simply be dropped.
enum TrackerOutput {
    /// Landmarks are available.
    Landmarks(LandmarkResultV2),
    /// Landmarks are unavailable (degraded tracking); we try to recover as best as we can from just
    /// the detection data.
    Detection(Detection),
}

/// The face track worker is sent the decoded webcam image and does the following:
///
/// - Detect faces (if none are currently tracked)
/// - Compute facial landmarks, track their positions across frames, and send them to the recipient
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
        .spawn(move |FaceTrackParams { image, output }| {
            if let Some(res) = tracker.track(&image) {
                output.fulfill((TrackerOutput::Landmarks(res.estimate().clone()), image));
            } else {
                // No ROI set, or tracking was lost. Run detection.

                // Zoom into the camera image and perform detection there. This makes outer
                // edges of the camera view unusable, but significantly improves the tracking
                // distance.
                let view_rect = image.resolution().fit_aspect_ratio(input_ratio);
                let view = image.view(view_rect);
                let detections = detector.detect(&view);

                if let Some(detection) = detections
                    .iter()
                    .max_by_key(|det| TotalF32(det.confidence()))
                {
                    // Adjust detection to be in the full image's coordinate space.
                    let mut detection = detection.clone();
                    detection
                        .set_bounding_rect(detection.bounding_rect().move_by(view_rect.top_left()));

                    // Tell tracker where to look.
                    let rect = RotatedRect::new(detection.bounding_rect(), detection.angle());
                    tracing::trace!("start tracking face at {:?}", rect);
                    tracker.set_roi(rect);

                    // Provide "degraded" tracking output to next stage.
                    output.fulfill((TrackerOutput::Detection(detection), image));
                }
            }
        })
}

type Filt = OneEuroFilter;
fn filter() -> TimedFilterAdapter<Filt> {
    Filt::new(0.0001, 0.3).real_time()
}
