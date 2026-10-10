//! Audio file playback via rodio (macOS + Windows).
//!
//! The single in-process playback path for all sounds: recording cues and the
//! settings "preview" button. Replaces the former per-platform process spawns
//! (afplay / PowerShell `SoundPlayer`) that lived in `paste`. Playing in the
//! long-lived backend process means a sound is never cut short by the overlay
//! WebView being torn down, and there is no decode/keep-alive jank. rodio
//! decodes via symphonia (mp3 out of the box) and handles resampling internally,
//! so no sample-rate matching is needed.

/// Play an audio file on a background thread. The thread blocks on
/// `sleep_until_end` so the `OutputStream` stays alive for the whole sound.
/// Errors are logged only — rodio + symphonia is stable enough that no fallback
/// (afplay/PowerShell) is warranted. Empty path is a no-op.
pub(crate) fn play_audio_file(file_path: &str) {
    if file_path.is_empty() {
        return;
    }
    let path = file_path.to_string();
    if let Err(error) = std::thread::Builder::new()
        .name("voicepaste-sound".to_string())
        .spawn(move || {
            if let Err(error) = play(&path) {
                log_app!(warn, "Audio playback failed ({}): {}", path, error);
            }
        })
    {
        log_app!(warn, "Failed to spawn audio thread: {}", error);
    }
}

fn play(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::File;
    use std::io::BufReader;
    // Keep `_stream` alive for the whole call: dropping it stops playback.
    let (_stream, handle) = rodio::OutputStream::try_default()?;
    let sink = rodio::Sink::try_new(&handle)?;
    let file = File::open(path)?;
    sink.append(rodio::Decoder::new(BufReader::new(file))?);
    sink.sleep_until_end();
    Ok(())
}

// ---------------------------------------------------------------------------
// Settings preview playback (stoppable, single-instance)
// ---------------------------------------------------------------------------

/// The settings preview currently playing, if any.
///
/// Recording cues stay fire-and-forget, but the history list's play button needs
/// to be stoppable and must never stack: clicking two entries in a row used to
/// start a second detached thread, so both recordings played at once with no way
/// to stop either. Only the rodio `Sink` is kept here — it is a `Send` handle,
/// whereas the `OutputStream` is not and has to stay on the thread that owns it.
fn preview_slot() -> &'static std::sync::Mutex<Option<std::sync::Arc<rodio::Sink>>> {
    static SLOT: std::sync::OnceLock<std::sync::Mutex<Option<std::sync::Arc<rodio::Sink>>>> =
        std::sync::OnceLock::new();
    SLOT.get_or_init(|| std::sync::Mutex::new(None))
}

/// Stop the preview currently playing, if any.
pub(crate) fn stop_preview() {
    let playing = preview_slot().lock().ok().and_then(|mut slot| slot.take());
    if let Some(sink) = playing {
        sink.stop();
    }
}

/// Play `file_path` as the settings preview, replacing whatever was playing.
/// `on_end` runs once, when playback finishes on its own or is stopped, so the
/// UI can return its button to the play state. Empty path is a no-op.
pub(crate) fn play_preview_file<F>(file_path: &str, on_end: F)
where
    F: FnOnce() + Send + 'static,
{
    stop_preview();
    if file_path.is_empty() {
        on_end();
        return;
    }

    // The callback has to be reachable from both the playback thread and the
    // spawn-failure path here, and it can only run once — hence the shared slot
    // rather than passing it straight into the closure. Whichever side gets
    // there first takes it; the UI is never left stuck on "playing".
    type EndCallback = std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>;
    fn fire(slot: &EndCallback) {
        let callback = slot.lock().ok().and_then(|mut slot| slot.take());
        if let Some(callback) = callback {
            callback();
        }
    }

    let on_end: std::sync::Arc<EndCallback> =
        std::sync::Arc::new(std::sync::Mutex::new(Some(Box::new(on_end))));
    let thread_on_end = std::sync::Arc::clone(&on_end);
    let path = file_path.to_string();
    if let Err(error) = std::thread::Builder::new()
        .name("voicepaste-preview".to_string())
        .spawn(move || {
            if let Err(error) = play_preview(&path) {
                log_app!(warn, "Preview playback failed ({}): {}", path, error);
            }
            fire(&thread_on_end);
        })
    {
        log_app!(warn, "Failed to spawn preview thread: {}", error);
        fire(&on_end);
    }
}

fn play_preview(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::File;
    use std::io::BufReader;
    // Keep `_stream` alive for the whole call: dropping it stops playback.
    let (_stream, handle) = rodio::OutputStream::try_default()?;
    let sink = std::sync::Arc::new(rodio::Sink::try_new(&handle)?);
    sink.append(rodio::Decoder::new(BufReader::new(File::open(path)?))?);

    if let Ok(mut slot) = preview_slot().lock() {
        *slot = Some(std::sync::Arc::clone(&sink));
    }
    sink.sleep_until_end();

    // Clear the slot only if it still holds *this* sink: a newer preview may
    // have replaced it while this one was winding down.
    if let Ok(mut slot) = preview_slot().lock() {
        let is_current = slot
            .as_ref()
            .is_some_and(|current| std::sync::Arc::ptr_eq(current, &sink));
        if is_current {
            *slot = None;
        }
    }
    Ok(())
}
