use std::{thread, time::Duration};

use not_news_audio::{SpeechEvent, SpeechSubmit, SpeechWorker};

fn main() -> Result<(), String> {
    let scratch = std::env::temp_dir().join(format!("not-news-voice-smoke-{}", std::process::id()));
    let mut worker = SpeechWorker::from_environment(&scratch);
    worker.reset_session();
    match worker.submit_note(
        "Kokoro voice integration verified.",
        std::time::Instant::now(),
    ) {
        SpeechSubmit::Queued => {}
        other => return Err(format!("voice smoke was not queued: {other:?}")),
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(70);
    loop {
        match worker.try_recv() {
            Ok(SpeechEvent::Played) => break,
            Ok(other) => return Err(format!("voice smoke failed: {other:?}")),
            Err(std::sync::mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("voice smoke worker stopped: {error}")),
        }
    }
    if scratch.exists()
        && scratch
            .read_dir()
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
    {
        return Err("voice smoke left synthesized audio behind".into());
    }
    let _ = std::fs::remove_dir(&scratch);
    Ok(())
}
