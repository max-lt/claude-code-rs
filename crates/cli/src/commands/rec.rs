use std::io::Cursor;
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::CommandResult;

/// Run voice recording outside of TUI raw mode.
/// This function temporarily disables raw mode, records, transcribes, and prompts for edits.
pub async fn run() -> Result<CommandResult> {
    let api_key =
        std::env::var("MISTRAL_API_KEY").map_err(|_| anyhow!("MISTRAL_API_KEY not set"))?;

    // Temporarily leave raw mode for recording
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
    )?;

    println!("🎤 Recording… (press Enter to stop)");
    let (samples, sample_rate) = record_audio()?;
    let wav = encode_wav(&samples, sample_rate)?;

    println!("✨ Transcribing…");
    let text = transcribe(&api_key, wav).await?;

    let final_text: String = dialoguer::Input::new()
        .with_prompt("Edit transcription")
        .with_initial_text(&text)
        .interact_text()?;

    // Return to TUI mode - the caller will re-enable raw mode
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
    )?;
    crossterm::terminal::enable_raw_mode()?;

    Ok(CommandResult::SendMessage(final_text))
}

fn record_audio() -> Result<(Vec<f32>, u32)> {
    let host = cpal::default_host();

    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("No input device available"))?;

    let config = device.default_input_config()?;
    let sample_rate = config.sample_rate();
    let sample_format = config.sample_format();
    let channels = config.channels() as usize;

    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let samples_clone = Arc::clone(&samples);

    let err_fn = |err: cpal::StreamError| {
        eprintln!("Stream error: {err}");
    };

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let mono = to_mono(data, channels);
                samples_clone.lock().unwrap().extend_from_slice(&mono);
            },
            err_fn,
            None,
        )?,
        SampleFormat::I16 => {
            let samples_clone = Arc::clone(&samples);
            device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let floats: Vec<f32> =
                        data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                    let mono = to_mono(&floats, channels);
                    samples_clone.lock().unwrap().extend_from_slice(&mono);
                },
                err_fn,
                None,
            )?
        }
        fmt => return Err(anyhow!("Unsupported sample format: {fmt:?}")),
    };

    stream.play()?;

    // Block until user presses Enter
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;

    drop(stream);

    let samples = Arc::try_unwrap(samples)
        .map_err(|_| anyhow!("Failed to unwrap samples"))?
        .into_inner()?;

    Ok((samples, sample_rate))
}

fn to_mono(data: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return data.to_vec();
    }

    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::new(&mut cursor, spec)?;

    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let value = (clamped * i16::MAX as f32) as i16;
        writer.write_sample(value)?;
    }

    writer.finalize()?;

    Ok(cursor.into_inner())
}

async fn transcribe(api_key: &str, wav_data: Vec<u8>) -> Result<String> {
    let part = reqwest::multipart::Part::bytes(wav_data)
        .file_name("recording.wav")
        .mime_str("audio/wav")?;

    let form = reqwest::multipart::Form::new()
        .text("model", "voxtral-mini-2602")
        .part("file", part);

    let client = reqwest::Client::new();

    let resp = client
        .post("https://api.mistral.ai/v1/audio/transcriptions")
        .header("x-api-key", api_key)
        .multipart(form)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Transcription failed ({status}): {body}"));
    }

    let json: serde_json::Value = resp.json().await?;

    json["text"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("No 'text' field in transcription response"))
}
