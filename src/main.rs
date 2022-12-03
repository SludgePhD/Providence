use std::collections::VecDeque;
use std::{cmp, io};

use pawawwewism::{promise, Promise, PromiseHandle, Worker};
use providence::data::TrackingMessage;
use providence::net::Publisher;
use providence::triangulate::Triangulator;
use zaru::face::detection::Detector;
use zaru::face::eye::{EyeLandmarker, EyeLandmarks};
use zaru::face::landmark::mediapipe_facemesh::{self, LandmarkResult, Landmarker};
use zaru::filter::ema::Ema;
use zaru::image::{Image, RotatedRect};
use zaru::landmark::{LandmarkFilter, LandmarkTracker};
use zaru::num::TotalF32;
use zaru::procrustes::ProcrustesAnalyzer;
use zaru::resolution::AspectRatio;
use zaru::timer::{FpsCounter, Timer};
use zaru::webcam::{ParamPreference, Webcam, WebcamOptions};
use zaru::Error;

fn main() -> Result<(), Error> {
    zaru::init_logger!();

    let eye_input_aspect = EyeLandmarker::new()
        .input_resolution()
        .aspect_ratio()
        .unwrap();

    let mut face_tracker = face_track_worker(eye_input_aspect)?;
    let mut left_eye_worker = eye_worker(Eye::Left)?;
    let mut right_eye_worker = eye_worker(Eye::Right)?;
    let mut assembler = assembler()?;

    let mut webcam = Webcam::open(
        WebcamOptions::default()
            .fps(30)
            .prefer(ParamPreference::Resolution),
    )?;

    let mut publisher = Publisher::spawn()?;
    let mut message_queue = VecDeque::new();
    let mut fps = FpsCounter::new("webcam");
    loop {
        // NB: the non-flipped webcam image is "the wrong way around" - but flipping the whole image
        // is *very* expensive for some reason, so we only flip the final result.
        let image = webcam.read()?;

        let (landmarks, landmarks_handle) = promise();
        let (left_eye, left_eye_handle) = promise();
        let (right_eye, right_eye_handle) = promise();
        let (left_eye_lm, left_eye_lm_handle) = promise();
        let (right_eye_lm, right_eye_lm_handle) = promise();
        let (message, message_handle) = promise();
        face_tracker.send(FaceTrackParams {
            image,
            landmarks,
            left_eye,
            right_eye,
        });
        left_eye_worker.send(EyeParams {
            eye_image: left_eye_handle,
            landmarks: left_eye_lm,
        });
        right_eye_worker.send(EyeParams {
            eye_image: right_eye_handle,
            landmarks: right_eye_lm,
        });
        assembler.send(AssemblerParams {
            landmarks: landmarks_handle,
            left_eye: left_eye_lm_handle,
            right_eye: right_eye_lm_handle,
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
    landmarks: PromiseHandle<LandmarkResult>,
    left_eye: PromiseHandle<(EyeLandmarks, Image)>,
    right_eye: PromiseHandle<(EyeLandmarks, Image)>,
    message: Promise<TrackingMessage>,
}

fn assembler() -> Result<Worker<AssemblerParams>, io::Error> {
    let mut fps = FpsCounter::new("assembler");
    let mut t_procrustes = Timer::new("procrustes");
    let mut t_triangulate = Timer::new("triangulate");

    let mut procrustes_analyzer =
        ProcrustesAnalyzer::new(mediapipe_facemesh::reference_positions());
    let mut tri = Triangulator::new();

    Worker::builder().name("assembler").spawn(
        move |AssemblerParams {
                  landmarks,
                  left_eye,
                  right_eye,
                  message,
              }| {
            let Ok(face_landmark) = landmarks.block() else { return };

            let procrustes_result = t_procrustes.time(|| {
                procrustes_analyzer.analyze(face_landmark.landmarks().positions().iter().map(
                    |&[x, y, z]| {
                        // Flip Y to bring us to canonical 3D coordinates (where Y points up).
                        // Only rotation matters, so we don't have to correct for the added
                        // translation.
                        (x, -y, z)
                    },
                ))
            });

            let quat = procrustes_result.rotation();
            let head_rotation = [quat.i, quat.j, quat.k, quat.w];

            let Ok((left, left_img)) = left_eye.block() else { return };
            let Ok((right, right_img)) = right_eye.block() else { return };

            // Up until now, we were using the webcam images as-is (non-mirrored), so from the users
            // perspective left and right eye are swapped and their textures flipped. Fix that now.
            let (mut left_img, mut right_img) = (right_img, left_img);
            left_img.flip_horizontal_in_place();
            right_img.flip_horizontal_in_place();
            let (mut left, mut right) = (right, left);
            left.flip_horizontal_in_place();
            right.flip_horizontal_in_place();
            // Face landmarks have been adjusted to be in range 0..1 earlier.
            let [x, y, _] = face_landmark.landmarks().average();
            let head_position = [1.0 - x, y];

            let guard = t_triangulate.start();
            let Ok(left_eye) = tri.triangulate_eye(&left, &left_img, true) else { return };
            let Ok(right_eye) = tri.triangulate_eye(&right, &right_img, false) else { return };
            drop(guard);
            message.fulfill(TrackingMessage {
                head_position,
                head_rotation,
                left_eye,
                right_eye,
            });

            fps.tick_with([&t_triangulate]);
        },
    )
}

struct FaceTrackParams {
    image: Image,
    landmarks: Promise<LandmarkResult>,
    left_eye: Promise<(Image, RotatedRect)>,
    right_eye: Promise<(Image, RotatedRect)>,
}

/// The face track worker is sent the decoded webcam image and does the following:
///
/// - Detect faces (if none are currently tracked)
/// - Compute facial landmarks and track their positions across frames
/// - Copy the eye's regions from the image and send them to the eye tracking workers
fn face_track_worker(eye_input_aspect: AspectRatio) -> Result<Worker<FaceTrackParams>, io::Error> {
    let mut fps = FpsCounter::new("tracker");
    let mut t_total = Timer::new("total");

    let mut detector = Detector::default();
    let mut landmarker = Landmarker::new();
    let mut tracker = LandmarkTracker::new(landmarker.input_resolution().aspect_ratio().unwrap());
    let input_ratio = detector.input_resolution().aspect_ratio().unwrap();

    Worker::builder().name("face tracker").spawn(
        move |FaceTrackParams {
                  image,
                  landmarks,
                  left_eye,
                  right_eye,
              }| {
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
                    let rect = target
                        .bounding_rect_loose()
                        .move_by(view_rect.x(), view_rect.y());
                    log::trace!("start tracking face at {:?}", rect);
                    tracker.set_roi(rect);
                }
            }

            if let Some(res) = tracker.track(&mut landmarker, &image) {
                let max = cmp::max(image.width(), image.height()) as f32;
                let mut lms = res.estimation().clone();
                lms.landmarks_mut()
                    .map_positions(|[x, y, z]| [x / max, y / max, z / max]);

                landmarks.fulfill(lms);

                let left = res.estimation().left_eye();
                let right = res.estimation().right_eye();

                const MARGIN: f32 = 0.9;
                let left = left.grow_rel(MARGIN).grow_to_fit_aspect(eye_input_aspect);
                let right = right.grow_rel(MARGIN).grow_to_fit_aspect(eye_input_aspect);

                left_eye.fulfill((image.view(left).to_image(), left));
                right_eye.fulfill((image.view(right).to_image(), right));
            }

            drop(guard);
            fps.tick_with(
                [&t_total]
                    .into_iter()
                    .chain(detector.timers())
                    .chain(landmarker.timers()),
            );
        },
    )
}

enum Eye {
    Left,
    Right,
}

struct EyeParams {
    eye_image: PromiseHandle<(Image, RotatedRect)>,
    landmarks: Promise<(EyeLandmarks, Image)>,
}

/// The eye tracking worker is passed a cropped image region by the face tracking worker and
/// computes eye and iris landmarks.
fn eye_worker(eye: Eye) -> Result<Worker<EyeParams>, io::Error> {
    let name = match eye {
        Eye::Left => "left iris",
        Eye::Right => "right iris",
    };
    let mut landmarker = EyeLandmarker::new();
    let mut fps = FpsCounter::new(name);
    let mut filter = LandmarkFilter::new(Ema::new(0.5), 76);

    Worker::builder().name(name).spawn(
        move |EyeParams {
                  eye_image,
                  landmarks,
              }| {
            let (image, _rect) = match eye_image.block() {
                Ok(v) => v,
                Err(_) => return,
            };
            let marks = match eye {
                Eye::Left => landmarker.compute(&image),
                Eye::Right => {
                    let marks = landmarker.compute(&image.flip_horizontal());
                    marks.flip_horizontal_in_place();
                    marks
                }
            };

            filter.filter(marks.landmarks_mut());

            landmarks.fulfill((marks.clone(), image));

            fps.tick_with(landmarker.timers());
        },
    )
}
