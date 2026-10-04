//! Beat detection and tempo against the synthesized track, whose kick times are known, analysed
//! frame by frame exactly as the live loop does it.

use mviz::analysis::Settings;
use mviz::demo::track;
use mviz::offline::walk;

/// Runs the analyser over a track at `fps` and returns (beat times, tempo estimates).
fn detect(bpm: f64, fps: f64) -> (Vec<f64>, Vec<f64>, Vec<f32>) {
    let t = track(bpm, 16, 48_000);
    let mono = t.mono();
    let mut beats = Vec::new();
    let mut tempos = Vec::new();
    let dur = mono.len() as f64 / 48_000.0;
    walk(&mono, 48_000, &Settings::default(), fps, 0.0, dur - 1.0, |f, _| {
        if f.beat {
            beats.push(f.time);
        }
        if let Some(b) = f.bpm {
            tempos.push(b);
        }
    });
    (t.kicks, beats, tempos)
}

/// Matches detections to kicks within `tol` seconds; returns (true positives, false positives,
/// misses, worst timing error of the matches).
fn score(kicks: &[f64], beats: &[f64], tol: f64) -> (usize, usize, usize, f64) {
    let mut used = vec![false; beats.len()];
    let mut tp = 0;
    let mut worst: f64 = 0.0;
    for &k in kicks {
        if let Some((i, &b)) = beats
            .iter()
            .enumerate()
            .filter(|(i, b)| !used[*i] && (**b - k).abs() <= tol)
            .min_by(|a, b| (a.1 - k).abs().total_cmp(&(b.1 - k).abs()))
        {
            used[i] = true;
            tp += 1;
            worst = worst.max((b - k).abs());
        }
    }
    (tp, beats.len() - tp, kicks.len() - tp, worst)
}

#[test]
fn beats_are_found_on_the_kicks() {
    for bpm in [90.0, 124.0, 140.0] {
        let (kicks, beats, _) = detect(bpm, 60.0);
        // Ignore the first second, where the threshold has no history yet.
        let kicks: Vec<f64> = kicks.into_iter().filter(|&k| k > 1.0 && k < 16.0 * 4.0 * 60.0 / bpm - 1.0).collect();
        let beats: Vec<f64> = beats.into_iter().filter(|&b| b > 1.0).collect();
        let (tp, fp, miss, worst) = score(&kicks, &beats, 0.05);
        let recall = tp as f64 / kicks.len() as f64;
        let precision = tp as f64 / (tp + fp).max(1) as f64;
        eprintln!("{bpm} BPM: {tp} found, {fp} extra, {miss} missed, worst offset {:.0} ms", worst * 1e3);
        assert!(recall >= 0.95, "{bpm} BPM: recall {recall:.2}");
        assert!(precision >= 0.95, "{bpm} BPM: precision {precision:.2}");
        // A frame is 16.7 ms and the onset window 21 ms: detections land within about two frames.
        assert!(worst <= 0.05);
    }
}

#[test]
fn tempo_is_estimated() {
    for bpm in [90.0, 124.0, 140.0] {
        let (_, _, tempos) = detect(bpm, 60.0);
        let last = *tempos.last().expect("a tempo after a few beats");
        assert!((last as f64 - bpm).abs() / bpm < 0.03, "{bpm} BPM read as {last}");
    }
}

#[test]
fn silence_has_no_beats_and_a_low_level() {
    let silence = vec![0.0f32; 48_000 * 5];
    let mut beats = 0;
    let mut last_db = 0.0;
    walk(&silence, 48_000, &Settings::default(), 60.0, 0.0, 4.0, |f, _| {
        beats += f.beat as usize;
        last_db = f.rms_db;
    });
    assert_eq!(beats, 0);
    assert!(last_db < -200.0);
}

#[test]
fn a_frame_rate_change_keeps_detection_working() {
    // The live loop runs at the display's rate; 30 and 144 frames per second must also work.
    for fps in [30.0, 144.0] {
        let (kicks, beats, _) = detect(124.0, fps);
        let kicks: Vec<f64> = kicks.into_iter().filter(|&k| k > 1.0 && k < 30.0).collect();
        let beats: Vec<f64> = beats.into_iter().filter(|&b| b > 1.0 && b < 30.5).collect();
        let (tp, fp, _, _) = score(&kicks, &beats, 1.5 / fps + 0.03);
        assert!(tp as f64 / kicks.len() as f64 >= 0.9, "{fps} fps: {tp} of {}", kicks.len());
        assert!(fp <= kicks.len() / 10, "{fps} fps: {fp} extra");
    }
}
