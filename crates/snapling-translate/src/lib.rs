//! Offline ru↔en translation: OPUS-MT (Helsinki-NLP, CC-BY-4.0) exported to ONNX by Xenova,
//! run with ONNX Runtime. Greedy decoding without a KV cache: fine for OCR-sized texts.

pub mod paddle;

use ort::session::Session;
use ort::value::Tensor;
use snapling_core::config::Online;
use snapling_core::i18n::{t, tf};
use std::path::{Path, PathBuf};
use tokenizers::Tokenizer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    RuEn,
    EnRu,
}

impl Dir {
    /// Model folder name under the models directory.
    pub fn model(self) -> &'static str {
        match self {
            Dir::RuEn => "opus-mt-ru-en",
            Dir::EnRu => "opus-mt-en-ru",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Dir::RuEn => t("Русский → English"),
            Dir::EnRu => t("English → Русский"),
        }
    }
}

/// More Cyrillic than Latin letters → ru→en.
pub fn detect(text: &str) -> Dir {
    let (mut cyr, mut lat) = (0, 0);
    for c in text.chars() {
        match c {
            'а'..='я' | 'А'..='Я' | 'ё' | 'Ё' => cyr += 1,
            'a'..='z' | 'A'..='Z' => lat += 1,
            _ => {}
        }
    }
    if cyr > lat { Dir::RuEn } else { Dir::EnRu }
}

/// OCR lines → sentences-ish: a line is glued to the next one when it does not end a
/// sentence and the next starts lowercase (a wrapped sentence), otherwise kept apart
/// (menus, labels).
pub fn segments(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let glue = out.last().is_some_and(|prev| {
            !prev.ends_with(['.', '!', '?', ':', ';', '…'])
                && line.chars().next().is_some_and(char::is_lowercase)
        });
        match out.last_mut() {
            Some(prev) if glue => {
                if prev.ends_with('-') {
                    prev.pop();
                } else {
                    prev.push(' ');
                }
                prev.push_str(line);
            }
            _ => out.push(line.to_string()),
        }
    }
    out
}

/// Files a model folder must contain, with pinned SHA-256 for the downloader.
pub const FILES: [&str; 3] = [
    "encoder_model_quantized.onnx",
    "decoder_model_quantized.onnx",
    "tokenizer.json",
];

pub fn sha256(dir: Dir, file: &str) -> Option<&'static str> {
    Some(match (dir, file) {
        (Dir::RuEn, "encoder_model_quantized.onnx") => {
            "c10df0466540534bb70c4402d539c349b8bfa9968184cad89f51408cf13e7eb7"
        }
        (Dir::RuEn, "decoder_model_quantized.onnx") => {
            "d880abf58612013815554ef55c98d0f28b68190b2cb6f65c39d6e834f51dda8c"
        }
        (Dir::EnRu, "encoder_model_quantized.onnx") => {
            "97f28d8295d231b7da721ded06fa8c034787df2122cd8f85b904abdc4d15bd53"
        }
        (Dir::EnRu, "decoder_model_quantized.onnx") => {
            "7e4e602c31130176b086cd4bf31d6d08d44a6a367617e910870e0ea3e5408574"
        }
        (_, "tokenizer.json") => "981cd3d9fef6bb4dda8082afda716b65deb6717cb3ef35ac0b57002c09dec2bb",
        _ => return None,
    })
}

/// Hugging Face URL of a model file.
pub fn url(dir: Dir, file: &str) -> String {
    let sub = if file.ends_with(".onnx") { "onnx/" } else { "" };
    format!(
        "https://huggingface.co/Xenova/{}/resolve/main/{sub}{file}",
        dir.model()
    )
}

pub fn installed(models: &Path, dir: Dir) -> bool {
    FILES
        .iter()
        .all(|f| models.join(dir.model()).join(f).is_file())
}

fn agent() -> ureq::Agent {
    agent_via(None)
}

fn agent_via(proxy: Option<ureq::Proxy>) -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .proxy(proxy)
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::NativeTls)
                    .build(),
            )
            .timeout_global(Some(std::time::Duration::from_secs(20)))
            .build(),
    )
}

/// `socks5://`, `socks4://`, `http://` or `https://`, login and password allowed.
/// The message never repeats the address: it may hold a password.
pub fn parse_proxy(url: &str) -> Result<ureq::Proxy, String> {
    ureq::Proxy::new(url.trim()).map_err(|_| t("Адрес прокси не распознан").to_string())
}

/// Header name and value.
type Auth = (&'static str, String);

/// Free-plan keys end with `:fx` and use a separate host.
pub fn deepl_host(key: &str) -> &'static str {
    if key.trim().ends_with(":fx") {
        "api-free.deepl.com"
    } else {
        "api.deepl.com"
    }
}

/// Url, auth header and JSON body of one request. The key travels in a header, never in the url.
fn request(service: Online, key: &str, text: &str, dir: Dir) -> Option<(String, Auth, String)> {
    let key = key.trim();
    let (from, to) = match dir {
        Dir::RuEn => ("ru", "en"),
        Dir::EnRu => ("en", "ru"),
    };
    Some(match service {
        Online::Off => return None,
        Online::Deepl => {
            let (source, target) = match dir {
                Dir::RuEn => ("RU", "EN-US"),
                Dir::EnRu => ("EN", "RU"),
            };
            (
                format!("https://{}/v2/translate", deepl_host(key)),
                ("Authorization", format!("DeepL-Auth-Key {key}")),
                serde_json::json!({ "text": [text], "source_lang": source, "target_lang": target })
                    .to_string(),
            )
        }
        Online::Yandex => (
            "https://translate.api.cloud.yandex.net/translate/v2/translate".into(),
            ("Authorization", format!("Api-Key {key}")),
            serde_json::json!({
                "texts": [text],
                "sourceLanguageCode": from,
                "targetLanguageCode": to,
            })
            .to_string(),
        ),
        Online::Google => (
            "https://translation.googleapis.com/language/translate/v2".into(),
            ("X-goog-api-key", key.to_string()),
            serde_json::json!({ "q": text, "source": from, "target": to, "format": "text" })
                .to_string(),
        ),
    })
}

fn reply(service: Online, json: &serde_json::Value) -> Option<String> {
    let text = match service {
        Online::Google => &json["data"]["translations"][0]["translatedText"],
        _ => &json["translations"][0]["text"],
    };
    text.as_str().map(str::to_string)
}

/// Online translation. Sends `text` to the service: call only on the user's explicit action.
/// Errors never contain the key or the proxy address.
pub fn online(
    service: Online,
    key: &str,
    text: &str,
    dir: Dir,
    proxy: Option<&str>,
) -> Result<String, String> {
    let name = service.name();
    let (url, (header, auth), body) =
        request(service, key, text, dir).ok_or(t("Онлайн-перевод выключен"))?;
    let via = proxy.map(parse_proxy).transpose()?;
    let through = via.is_some();
    let resp = agent_via(via)
        .post(url)
        .header(header, auth)
        .content_type("application/json")
        .send(body);
    let mut resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(code)) => {
            return Err(match code {
                401 | 403 => tf("{0}: ключ не подходит", &[("0", &name)]),
                // Google answers 400 to a wrong key; the body here is always well formed.
                400 if service == Online::Google => tf("{0}: ключ не подходит", &[("0", &name)]),
                456 => tf("{0}: исчерпан лимит символов", &[("0", &name)]),
                429 => tf(
                    "{0}: слишком много запросов, попробуйте позже",
                    &[("0", &name)],
                ),
                c => tf("{0}: ошибка сервера {c}", &[("0", &name), ("c", &c)]),
            });
        }
        Err(_) if through => return Err(tf("{0}: нет связи через прокси", &[("0", &name)])),
        Err(e) => return Err(tf("{0}: нет связи ({e})", &[("0", &name), ("e", &e)])),
    };
    let json: serde_json::Value =
        serde_json::from_str(&resp.body_mut().read_to_string().map_err(err)?).map_err(err)?;
    reply(service, &json).ok_or_else(|| tf("{0}: неожиданный ответ", &[("0", &name)]))
}

/// Download size of one direction, for the consent prompt.
pub const DOWNLOAD_MB: u32 = 117;

/// Fetches the model files from Hugging Face, checking each against its pinned SHA-256.
/// The only network access of the offline translator; called only after the user agreed.
pub fn download(models: &Path, dir: Dir) -> Result<(), String> {
    let root = models.join(dir.model());
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    for file in FILES {
        let want = sha256(dir, file).ok_or(t("нет контрольной суммы"))?;
        fetch_file(&url(dir, file), &root.join(file), want)?;
    }
    Ok(())
}

/// Downloads `url` to `dest` unless it is there; a wrong SHA-256 discards the file.
pub(crate) fn fetch_file(url: &str, dest: &Path, want: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    if dest.is_file() {
        return Ok(());
    }
    let file = dest
        .file_name()
        .map_or_else(String::new, |f| f.to_string_lossy().to_string());
    let part = dest.with_extension("part");
    let fetch = || -> Result<(), String> {
        let resp = agent()
            .get(url)
            .call()
            .map_err(|e| format!("{file}: {e}"))?;
        let mut body = resp.into_body().into_reader();
        let mut out = std::fs::File::create(&part).map_err(err)?;
        let mut hash = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = body.read(&mut buf).map_err(|e| format!("{file}: {e}"))?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(err)?;
        }
        let got: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got != want {
            return Err(tf(
                "{file}: контрольная сумма не совпала, файл отброшен",
                &[("file", &file)],
            ));
        }
        drop(out);
        std::fs::rename(&part, dest).map_err(err)
    };
    let res = fetch();
    if res.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    res
}

const EOS: i64 = 0;
/// Also `decoder_start_token_id`; never generated (`bad_words_ids`).
const PAD: i64 = 62517;

pub struct Model {
    enc: Session,
    dec: Session,
    tok: Tokenizer,
}

pub(crate) fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// ONNX Runtime allows one environment per process.
fn env() -> Result<&'static ort::environment::Environment, String> {
    static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> =
        std::sync::OnceLock::new();
    ENV.get_or_init(|| {
        ort::init()
            .with_name("snapling")
            .with_telemetry(false)
            .build()
            .map_err(err)
    })
    .as_ref()
    .map_err(Clone::clone)
}

pub(crate) fn session(file: &Path) -> Result<Session, String> {
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(4));
    Session::builder(env()?)
        .map_err(err)?
        .with_intra_threads(threads)
        .map_err(err)?
        // Every decoding step has a new shape: the arena and memory patterns only grow.
        .with_memory_pattern(false)
        .map_err(err)?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(err)?
        .commit_from_file(file)
        .map_err(|e| format!("{}: {e}", file.display()))
}

/// Xenova's export has `"precompiled_charsmap": null`, which `tokenizers` rejects;
/// an empty charsmap is a no-op normalizer, so drop it.
fn tokenizer(path: &Path) -> Result<Tokenizer, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut json: serde_json::Value = serde_json::from_str(&text).map_err(err)?;
    if json["normalizer"]["precompiled_charsmap"].is_null() {
        json["normalizer"] = serde_json::Value::Null;
    }
    Tokenizer::from_bytes(json.to_string()).map_err(err)
}

impl Model {
    pub fn load(models: &Path, dir: Dir) -> Result<Self, String> {
        let root: PathBuf = models.join(dir.model());
        if !installed(models, dir) {
            return Err(tf(
                "модель перевода не установлена: {0}",
                &[("0", &root.display())],
            ));
        }
        Ok(Self {
            enc: session(&root.join(FILES[0]))?,
            dec: session(&root.join(FILES[1]))?,
            tok: tokenizer(&root.join(FILES[2]))?,
        })
    }

    pub fn translate(&mut self, text: &str) -> Result<String, String> {
        let mut out = Vec::new();
        for s in segments(text) {
            out.push(self.sentence(&s)?);
        }
        Ok(out.join("\n"))
    }

    fn sentence(&mut self, text: &str) -> Result<String, String> {
        let enc = self.tok.encode(text, true).map_err(err)?;
        let ids: Vec<i64> = enc
            .get_ids()
            .iter()
            .map(|&i| i64::from(i))
            .take(500)
            .collect();
        let n = ids.len();
        let mask = vec![1i64; n];
        let hidden = {
            let out = self
                .enc
                .run(ort::inputs![
                    "input_ids" => Tensor::from_array(([1, n], ids)).map_err(err)?,
                    "attention_mask" => Tensor::from_array(([1, n], mask.clone())).map_err(err)?,
                ])
                .map_err(err)?;
            let (shape, data) = out["last_hidden_state"]
                .try_extract_tensor::<f32>()
                .map_err(err)?;
            (shape.to_vec(), data.to_vec())
        };
        let mut out_ids = vec![PAD];
        let max = (n * 2 + 16).min(512);
        while out_ids.len() < max {
            let t = out_ids.len();
            let out = self
                .dec
                .run(ort::inputs![
                    "input_ids" => Tensor::from_array(([1, t], out_ids.clone())).map_err(err)?,
                    "encoder_attention_mask" => Tensor::from_array(([1, n], mask.clone())).map_err(err)?,
                    "encoder_hidden_states" => Tensor::from_array((hidden.0.clone(), hidden.1.clone())).map_err(err)?,
                ])
                .map_err(err)?;
            let (shape, logits) = out["logits"].try_extract_tensor::<f32>().map_err(err)?;
            let vocab = shape[2] as usize;
            let last = &logits[(t - 1) * vocab..t * vocab];
            let next = last
                .iter()
                .enumerate()
                .filter(|&(i, _)| i as i64 != PAD)
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map_or(EOS, |(i, _)| i as i64);
            if next == EOS {
                break;
            }
            out_ids.push(next);
        }
        let ids: Vec<u32> = out_ids[1..].iter().map(|&i| i as u32).collect();
        self.tok.decode(&ids, true).map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_errors_do_not_repeat_the_address() {
        assert!(parse_proxy("socks5://127.0.0.1:1080").is_ok());
        assert!(parse_proxy("http://user:pass@proxy.example:8080").is_ok());
        let e = parse_proxy("ftp://user:secret@host").err().unwrap();
        assert!(!e.contains("secret"));
    }

    #[test]
    fn detects_direction() {
        assert_eq!(detect("Привет, мир"), Dir::RuEn);
        assert_eq!(detect("Hello, world"), Dir::EnRu);
        assert_eq!(detect("Открыть file"), Dir::RuEn);
        assert_eq!(detect("123"), Dir::EnRu);
    }

    #[test]
    fn glues_wrapped_lines_only() {
        assert_eq!(
            segments(
                "Auto-\nmatic save.\nЭто длинное предложение,\nкоторое перенеслось.\nФайл\nПравка"
            ),
            vec![
                "Automatic save.",
                "Это длинное предложение, которое перенеслось.",
                "Файл",
                "Правка"
            ]
        );
    }

    #[test]
    fn online_requests_keep_the_key_out_of_the_url() {
        for service in [Online::Deepl, Online::Yandex, Online::Google] {
            let (url, (_, auth), body) =
                request(service, " sec:fx\n", "привет", Dir::RuEn).unwrap();
            assert!(auth.ends_with("sec:fx"), "{auth}");
            assert!(
                !url.contains("sec") && !body.contains("sec"),
                "{url} {body}"
            );
            assert!(body.contains("привет"), "{body}");
        }
        assert!(request(Online::Off, "k", "t", Dir::RuEn).is_none());
    }

    #[test]
    fn online_replies_are_read() {
        let flat = serde_json::json!({ "translations": [{ "text": "hi" }] });
        let google =
            serde_json::json!({ "data": { "translations": [{ "translatedText": "hi" }] } });
        assert_eq!(reply(Online::Deepl, &flat).as_deref(), Some("hi"));
        assert_eq!(reply(Online::Yandex, &flat).as_deref(), Some("hi"));
        assert_eq!(reply(Online::Google, &google).as_deref(), Some("hi"));
        assert_eq!(reply(Online::Google, &flat), None);
    }

    #[test]
    fn deepl_host_by_key_plan() {
        assert_eq!(deepl_host("abc:fx"), "api-free.deepl.com");
        assert_eq!(deepl_host("abc"), "api.deepl.com");
    }

    #[test]
    fn urls_and_hashes_cover_all_files() {
        for d in [Dir::RuEn, Dir::EnRu] {
            for f in FILES {
                assert!(sha256(d, f).is_some_and(|h| h.len() == 64));
                assert!(url(d, f).starts_with("https://huggingface.co/Xenova/opus-mt-"));
            }
        }
        assert!(
            url(Dir::RuEn, "encoder_model_quantized.onnx")
                .ends_with("/resolve/main/onnx/encoder_model_quantized.onnx")
        );
    }

    /// Network: `SNAP_DL=<empty dir> cargo test -p snapling-translate download -- --ignored`
    #[test]
    #[ignore]
    fn download_verifies_hashes() {
        let dir = PathBuf::from(std::env::var("SNAP_DL").expect("SNAP_DL"));
        download(&dir, Dir::EnRu).unwrap();
        assert!(installed(&dir, Dir::EnRu));
        download(&dir, Dir::EnRu).unwrap();
    }

    /// Needs the models: `SNAP_MODELS=target/devdata/local/Snapling/models cargo test -p snapling-translate -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn translates_both_ways() {
        let models = PathBuf::from(std::env::var("SNAP_MODELS").expect("SNAP_MODELS"));
        for (dir, text) in [
            (Dir::RuEn, "Сохранить снимок экрана в папку.\nОтмена"),
            (Dir::EnRu, "Save the screenshot to a folder.\nCancel"),
        ] {
            let t0 = std::time::Instant::now();
            let mut m = Model::load(&models, dir).unwrap();
            let load = t0.elapsed();
            let got = m.translate(text).unwrap();
            println!("{dir:?}: load {load:?}, total {:?}\n{got}", t0.elapsed());
            assert_eq!(got.lines().count(), 2, "{got}");
        }
    }
}
