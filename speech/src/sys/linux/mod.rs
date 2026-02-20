use crate::{RecognitionConfig, RecognitionResult, SpeechError, TtsConfig, Voice};

pub const fn recognition_is_available() -> bool {
    false
}

#[derive(Debug)]
pub struct TtsInner;

impl TtsInner {
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Self, SpeechError> {
        // Verify speech-dispatcher is available
        let status = std::process::Command::new("which")
            .arg("spd-say")
            .output()
            .map_err(|e| SpeechError::PlatformError(e.to_string()))?;
        if !status.status.success() {
            return Err(SpeechError::NotAvailable);
        }
        Ok(Self)
    }

    pub fn available_voices(&self) -> Result<Vec<Voice>, SpeechError> {
        let _ = self;
        let output = std::process::Command::new("spd-say")
            .arg("-L")
            .output()
            .map_err(|e| SpeechError::PlatformError(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .skip(1)
            .filter(|l| !l.is_empty())
            .map(|line| {
                let parts: Vec<&str> = line.splitn(2, '\t').collect();
                Voice {
                    id: parts.first().unwrap_or(&"").to_string(),
                    name: parts.first().unwrap_or(&"").to_string(),
                    language: parts.get(1).unwrap_or(&"").to_string(),
                }
            })
            .collect())
    }

    pub async fn speak(&self, text: &str, config: &TtsConfig) -> Result<(), SpeechError> {
        let rate_percent = (config.rate - 1.0) * 100.0;
        let pitch_percent = (config.pitch - 1.0) * 100.0;
        let volume_percent = config.volume * 100.0;
        let mut cmd = async_process::Command::new("spd-say");
        cmd.arg("-r").arg(format!("{rate_percent:.0}"));
        cmd.arg("-p").arg(format!("{pitch_percent:.0}"));
        cmd.arg("-i").arg(format!("{volume_percent:.0}"));
        cmd.arg("-w"); // wait until done
        cmd.arg(text);
        let status = cmd
            .status()
            .await
            .map_err(|e| SpeechError::PlatformError(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(SpeechError::PlatformError("spd-say failed".into()))
        }
    }

    pub fn stop(&self) {
        let _ = self;
        let _ = std::process::Command::new("spd-say").arg("-S").spawn();
    }

    pub const fn is_speaking(&self) -> bool {
        let _ = self;
        false
    }
}

#[derive(Debug)]
pub struct SpeechRecognizerInner;

impl SpeechRecognizerInner {
    #[allow(clippy::unused_async)]
    pub async fn start(
        _config: RecognitionConfig,
    ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
        Err(SpeechError::NotSupported)
    }

    pub const fn stop(&self) {
        let _ = self;
    }
}
