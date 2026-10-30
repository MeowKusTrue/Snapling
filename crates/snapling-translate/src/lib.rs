//! Offline ru↔en translation: OPUS-MT (Helsinki-NLP, CC-BY-4.0) exported to ONNX by Xenova,
//! run with ONNX Runtime. Greedy decoding without a KV cache: fine for OCR-sized texts.

mod models;
pub mod paddle;

pub use models::{DIRECTIONS, Direction};

use ort::session::Session;
use ort::value::Tensor;
use snapling_core::config::Online;
use snapling_core::i18n::{n, t, tf};
use std::path::{Path, PathBuf};
use tokenizers::Tokenizer;

pub type Lang = &'static str;

/// Language code and name. Offline pairs are in [`DIRECTIONS`]; the rest needs an online service.
pub const LANGS: [(Lang, &str); 14] = [
    ("en", n("Английский")),
    ("ru", n("Русский")),
    ("de", n("Немецкий")),
    ("fr", n("Французский")),
    ("es", n("Испанский")),
    ("it", n("Итальянский")),
    ("pt", n("Португальский")),
    ("uk", n("Украинский")),
    ("pl", n("Польский")),
    ("nl", n("Нидерландский")),
    ("tr", n("Турецкий")),
    ("zh", n("Китайский")),
    ("ja", n("Японский")),
    ("ko", n("Корейский")),
];

pub fn lang_name(code: &str) -> &'static str {
    LANGS.iter().find(|l| l.0 == code).map_or("", |l| t(l.1))
}

/// Language by its alphabet. Latin scripts look alike, so they all read as English.
pub fn detect(text: &str) -> Lang {
    let (mut cyr, mut uk, mut lat, mut han, mut kana, mut hangul) = (0, 0, 0, 0, 0, 0);
    for c in text.chars() {
        match c {
            'і' | 'ї' | 'є' | 'ґ' | 'І' | 'Ї' | 'Є' | 'Ґ' => {
                cyr += 1;
                uk += 1;
            }
            '\u{400}'..='\u{4FF}' => cyr += 1,
            'a'..='z' | 'A'..='Z' => lat += 1,
            '\u{3040}'..='\u{30FF}' => kana += 1,
            '\u{AC00}'..='\u{D7A3}' => hangul += 1,
            '\u{4E00}'..='\u{9FFF}' => han += 1,
            _ => {}
        }
    }
    let top = [cyr, lat, han + kana, hangul]
        .into_iter()
        .max()
        .unwrap_or(0);
    match top {
        0 => "en",
        _ if hangul == top => "ko",
        _ if han + kana == top && kana > 0 => "ja",
        _ if han + kana == top => "zh",
        _ if cyr == top && uk > 0 => "uk",
        _ if cyr == top => "ru",
        _ => "en",
    }
}

/// Where a text in `from` goes: the wanted language, or English when it is already in it.
pub fn target_for(from: Lang, wanted: &str) -> Lang {
    let wanted = LANGS.iter().find(|l| l.0 == wanted).map_or("ru", |l| l.0);
    match (from == wanted, from) {
        (true, "en") => "ru",
        (true, _) => "en",
        _ => wanted,
    }
}

pub fn direction(from: &str, to: &str) -> Option<&'static Direction> {
    DIRECTIONS.iter().find(|d| d.from == from && d.to == to)
}

/// Models a pair needs: a direct one, or two through English. Empty: no offline way.
pub fn route(from: &str, to: &str) -> Vec<&'static Direction> {
    if let Some(d) = direction(from, to) {
        return vec![d];
    }
    match (direction(from, "en"), direction("en", to)) {
        (Some(a), Some(b)) => vec![a, b],
        _ => Vec::new(),
    }
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

/// Files a model folder must contain.
pub const FILES: [&str; 3] = [
    "encoder_model_quantized.onnx",
    "decoder_model_quantized.onnx",
    "tokenizer.json",
];

impl Direction {
    /// Model folder name under the models directory, and its name on Hugging Face.
    pub fn model(&self) -> String {
        format!("opus-mt-{}-{}", self.from, self.to)
    }

    fn sha256(&self, file: &str) -> &'static str {
        match file {
            "encoder_model_quantized.onnx" => self.encoder,
            "decoder_model_quantized.onnx" => self.decoder,
            _ => self.tokenizer,
        }
    }

    fn url(&self, file: &str) -> String {
        let sub = if file.ends_with(".onnx") { "onnx/" } else { "" };
        format!(
            "https://huggingface.co/Xenova/{}/resolve/main/{sub}{file}",
            self.model()
        )
    }

    pub fn installed(&self, models: &Path) -> bool {
        let root = models.join(self.model());
        FILES.iter().all(|f| root.join(f).is_file())
    }
}

fn agent_via(proxy: Option<ureq::Proxy>) -> ureq::Agent {
    agent_for(proxy, Some(std::time::Duration::from_secs(20)))
}

fn agent_for(proxy: Option<ureq::Proxy>, limit: Option<std::time::Duration>) -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .proxy(proxy)
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::NativeTls)
                    .build(),
            )
            .timeout_global(limit)
            .timeout_connect(Some(std::time::Duration::from_secs(20)))
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
fn request(
    service: Online,
    key: &str,
    text: &str,
    from: Lang,
    to: Lang,
) -> Option<(String, Auth, String)> {
    let key = key.trim();
    Some(match service {
        Online::Off => return None,
        Online::Deepl => {
            let source = from.to_uppercase();
            let target = match to {
                "en" => "EN-US".to_string(),
                "pt" => "PT-BR".to_string(),
                _ => to.to_uppercase(),
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
    from: Lang,
    to: Lang,
    proxy: Option<&str>,
) -> Result<String, String> {
    let name = service.name();
    let (url, (header, auth), body) =
        request(service, key, text, from, to).ok_or(t("Онлайн-перевод выключен"))?;
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

const RELEASES: &str = "https://api.github.com/repos/MeowKusTrue/Snapling/releases/latest";

/// `tag` is a later version than `current`; both like `v1.2.3`.
pub fn is_newer(tag: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u32> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parts(tag) > parts(current)
}

/// Latest published version and its page, `None` when nothing is published.
/// Sends nothing but the request itself; call only on the user's click.
pub fn latest_release() -> Result<Option<(String, String)>, String> {
    let resp = agent_via(None)
        .get(RELEASES)
        .header("User-Agent", "Snapling")
        .header("Accept", "application/vnd.github+json")
        .call();
    let mut resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(tf("Нет связи ({e})", &[("e", &e)])),
    };
    let json: serde_json::Value =
        serde_json::from_str(&resp.body_mut().read_to_string().map_err(err)?).map_err(err)?;
    let field = |k: &str| json[k].as_str().map(str::to_string);
    Ok(field("tag_name").zip(field("html_url")))
}

/// Fetches the model files from Hugging Face, checking each against its pinned SHA-256.
/// The only network access of the offline translator; called only after the user agreed.
pub fn download(models: &Path, d: &Direction) -> Result<(), String> {
    let root = models.join(d.model());
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    for file in FILES {
        fetch_file(&d.url(file), &root.join(file), d.sha256(file))?;
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
        let resp = agent_for(None, None)
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

pub struct Model {
    enc: Session,
    dec: Session,
    tok: Tokenizer,
    /// Also `decoder_start_token_id`; never generated (`bad_words_ids`).
    pad: i64,
    eos: i64,
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
    pub fn load(models: &Path, d: &Direction) -> Result<Self, String> {
        let root: PathBuf = models.join(d.model());
        if !d.installed(models) {
            return Err(tf(
                "модель перевода не установлена: {0}",
                &[("0", &root.display())],
            ));
        }
        Ok(Self {
            enc: session(&root.join(FILES[0]))?,
            dec: session(&root.join(FILES[1]))?,
            tok: tokenizer(&root.join(FILES[2]))?,
            pad: d.pad,
            eos: d.eos,
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
        let mut out_ids = vec![self.pad];
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
                .filter(|&(i, _)| i as i64 != self.pad)
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map_or(self.eos, |(i, _)| i as i64);
            if next == self.eos {
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
    fn detects_the_alphabet() {
        assert_eq!(detect("Привет, мир"), "ru");
        assert_eq!(detect("Привіт, світе"), "uk");
        assert_eq!(detect("Hello, world"), "en");
        assert_eq!(detect("Открыть file"), "ru");
        assert_eq!(detect("123"), "en");
        assert_eq!(detect("你好世界"), "zh");
        assert_eq!(detect("こんにちは世界"), "ja");
        assert_eq!(detect("안녕하세요"), "ko");
    }

    #[test]
    fn a_pair_goes_direct_or_through_english() {
        assert_eq!(route("ru", "en").len(), 1);
        let two = route("de", "ru");
        assert_eq!((two[0].from, two[1].to), ("de", "ru"));
        assert_eq!(two[0].to, two[1].from);
        assert!(route("en", "ja").is_empty(), "no such model");
        assert!(route("pt", "en").is_empty());
        for (code, _) in LANGS {
            assert!(!lang_name(code).is_empty());
        }
    }

    #[test]
    fn target_never_equals_the_source() {
        assert_eq!(target_for("en", "ru"), "ru");
        assert_eq!(target_for("ru", "ru"), "en");
        assert_eq!(target_for("en", "en"), "ru");
        assert_eq!(target_for("de", "xx"), "ru");
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("v0.1.0", "0.0.1"));
        assert!(is_newer("0.0.10", "0.0.9"));
        assert!(!is_newer("v0.0.1", "0.0.1"));
        assert!(!is_newer("v0.0.1", "0.1.0"));
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
                request(service, " sec:fx\n", "привет", "ru", "en").unwrap();
            assert!(auth.ends_with("sec:fx"), "{auth}");
            assert!(
                !url.contains("sec") && !body.contains("sec"),
                "{url} {body}"
            );
            assert!(body.contains("привет"), "{body}");
        }
        assert!(request(Online::Off, "k", "t", "ru", "en").is_none());
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
        for d in &DIRECTIONS {
            for f in FILES {
                assert_eq!(d.sha256(f).len(), 64);
                assert!(
                    d.url(f)
                        .starts_with("https://huggingface.co/Xenova/opus-mt-")
                );
            }
        }
        assert!(
            DIRECTIONS[0]
                .url("encoder_model_quantized.onnx")
                .ends_with("/resolve/main/onnx/encoder_model_quantized.onnx")
        );
    }

    /// Network, ~100 MB per direction:
    /// `SNAP_MODELS=<dir> cargo test --release -p snapling-translate pairs_translate -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn pairs_translate() {
        let models = PathBuf::from(std::env::var("SNAP_MODELS").expect("SNAP_MODELS"));
        for (from, to, text) in [
            (
                "ru",
                "en",
                "Сохранить снимок экрана в папку.
Отмена",
            ),
            (
                "en",
                "ru",
                "Save the screenshot to a folder.
Cancel",
            ),
            ("de", "ru", "Der Bildschirm wurde gespeichert."),
            ("en", "zh", "Save the screenshot to a folder."),
            ("zh", "en", "保存屏幕截图。"),
            ("ja", "en", "画面を保存しました。"),
            ("ko", "en", "화면을 저장했습니다."),
        ] {
            let mut text = text.to_string();
            let t0 = std::time::Instant::now();
            for d in route(from, to) {
                download(&models, d).unwrap();
                text = Model::load(&models, d).unwrap().translate(&text).unwrap();
            }
            println!("{from}-{to} {:?}: {text}", t0.elapsed());
            assert!(!text.is_empty());
        }
    }
}
