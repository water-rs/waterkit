use crate::{RecognitionConfig, RecognitionResult, SpeechError, TtsConfig, Voice};
use std::io::BufRead;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

const LINUX_RECOGNIZER_ENV: &str = "WATERKIT_SPEECH_LINUX_RECOGNIZER";
const LANGUAGE_PLACEHOLDER: &str = "{language}";

pub fn recognition_is_available() -> bool {
    std::env::var(LINUX_RECOGNIZER_ENV).is_ok_and(|value| !value.trim().is_empty())
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
            .map_err(|e| SpeechError::Platform(e.to_string()))?;
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
            .map_err(|e| SpeechError::Platform(e.to_string()))?;
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
            .map_err(|e| SpeechError::Platform(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(SpeechError::Platform("spd-say failed".into()))
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
pub struct SpeechRecognizerInner {
    child: Mutex<Option<std::process::Child>>,
    stopping: AtomicBool,
}

impl SpeechRecognizerInner {
    #[allow(clippy::unused_async)]
    pub async fn start(
        config: RecognitionConfig,
    ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
        let recognizer_command = std::env::var(LINUX_RECOGNIZER_ENV)
            .map_err(|_| SpeechError::NotAvailable)?
            .trim()
            .to_string();
        if recognizer_command.is_empty() {
            return Err(SpeechError::NotAvailable);
        }

        let mut child = Command::new("sh")
            .arg("-lc")
            .arg(build_linux_command(
                &recognizer_command,
                config.language.as_deref(),
            ))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|e| SpeechError::Platform(format!("spawn recognizer command: {e}")))?;

        let stdout = child.stdout.take().ok_or_else(|| {
            SpeechError::Platform("recognizer command did not expose stdout".into())
        })?;
        let (tx, rx) = async_channel::bounded(64);

        std::thread::Builder::new()
            .name("waterkit-speech-linux-recognizer".to_string())
            .spawn(move || {
                let reader = std::io::BufReader::new(stdout);
                for line_result in reader.lines() {
                    let Ok(line) = line_result else {
                        break;
                    };
                    let text = line.trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    let _ = tx.try_send(RecognitionResult {
                        text,
                        is_final: true,
                        confidence: None,
                    });
                }
                tx.close();
            })
            .map_err(|e| {
                SpeechError::Platform(format!("spawn recognizer reader thread: {e}"))
            })?;

        Ok((
            Self {
                child: Mutex::new(Some(child)),
                stopping: AtomicBool::new(false),
            },
            rx,
        ))
    }

    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Ok(mut child_guard) = self.child.lock()
            && let Some(child) = child_guard.as_mut()
        {
            let _ = child.kill();
        }
    }
}

impl Drop for SpeechRecognizerInner {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Ok(child_guard) = self.child.get_mut()
            && let Some(child) = child_guard.as_mut()
        {
            let _ = child.kill();
        }
    }
}

fn build_linux_command(command: &str, language: Option<&str>) -> String {
    let Some(language) = language else {
        return command.to_string();
    };
    command.replace(LANGUAGE_PLACEHOLDER, language)
}
