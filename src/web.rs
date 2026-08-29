//! Web access (Pilar 10 + 12, ROADMAP Fase 5): `web_search` + `fetch_url` + `generate_image`.
//!
//! - **web_search** — abstraction `SearchProvider` (dipilih via `SEARCH_PROVIDER`, pola
//!   sama seperti `AI_PROVIDER`); MVP primary: Tavily (konten LLM-ready, hemat token).
//! - **fetch_url** — chain 4-tier (AGENTS.md Pilar 10): direct fetch `Accept: text/markdown`
//!   → markdown.new → r.jina.ai (keyless) → plain HTML + strip lokal. SSRF guard via custom
//!   DNS resolver yang menolak IP private/reserved — berlaku juga utk redirect target.
//! - **generate_image** — Pollinations (gratis, tanpa key) → `send_photo`. Model hanya
//!   memanggil tool dengan prompt teks — tidak butuh vision (Pilar 12).
//!
//! Hygiene (Pilar 10): hanya URL publik, size cap per request + file attachment utk
//! konten panjang, timeout per request, no caching (evidence-driven — nanti kalau perlu).

use crate::config::Config;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InputFile};

/// Cap konten fetch_url yang masuk context LLM; versi penuh dikirim sebagai file (Pilar 10).
const FETCH_MAX_CTX_CHARS: usize = 15_000;
/// Batas download per request — cegah memory blow-up pada file raksasa.
const MAX_DOWNLOAD_BYTES: usize = 2 * 1024 * 1024;
/// Cap snippet per hasil search (jaga context LLM ramping).
const SNIPPET_MAX_CHARS: usize = 400;
/// Cap judul per hasil search.
const TITLE_MAX_CHARS: usize = 150;
/// Cap prompt image (mencegah URL terlalu panjang setelah encoding).
const IMAGE_PROMPT_MAX_CHARS: usize = 1000;
/// Timeout default pencarian.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);

const UA: &str = concat!(
    "HermesLite/",
    env!("CARGO_PKG_VERSION"),
    " (personal assistant; Telegram)"
);

pub struct WebCtx {
    pub bot: Bot,
    /// Provider pencarian aktif (SEARCH_PROVIDER).
    pub search: Arc<dyn SearchProvider>,
    /// HTTP client dengan DNS resolver SSRF-safe — untuk URL user-supplied (tier 1 & 4).
    pub safe_http: reqwest::Client,
    /// HTTP client biasa — untuk host pihak ketiga yang sudah diketahui aman
    /// (Tavily, markdown.new, r.jina.ai, Pollinations).
    pub plain_http: reqwest::Client,
    pub fetch_timeout: Duration,
    pub image_timeout: Duration,
}

impl WebCtx {
    pub fn new(cfg: &Config, bot: Bot) -> Arc<Self> {
        Arc::new(Self {
            bot,
            search: build_search_provider(cfg),
            // Tanpa timeout level client — per-request via RequestBuilder::timeout
            // (fetch, search, image punya budget beda).
            safe_http: reqwest::Client::builder()
                .dns_resolver(Arc::new(SafeResolver))
                .user_agent(UA)
                .build()
                .expect("build safe reqwest client"),
            plain_http: reqwest::Client::builder()
                .user_agent(UA)
                .build()
                .expect("build plain reqwest client"),
            fetch_timeout: Duration::from_secs(cfg.fetch_timeout),
            image_timeout: Duration::from_secs(cfg.image_timeout),
        })
    }

    // ── web_search ────────────────────────────────────────────────────────────

    pub async fn web_search(&self, query: &str, max_results: usize) -> Result<String> {
        let query = query.trim();
        if query.is_empty() {
            bail!("parameter 'query' wajib diisi");
        }
        let max = max_results.clamp(1, 10);
        let results = self.search.search(query, max).await?;
        if results.is_empty() {
            return Ok(format!("🔍 \"{query}\" — tidak ada hasil (via {}).", self.search.name()));
        }
        tracing::info!(query, n = results.len(), provider = self.search.name(), "web_search");
        Ok(format_search_results(self.search.name(), query, &results))
    }

    // ── fetch_url (chain 4-tier, Pilar 10) ────────────────────────────────────

    pub async fn fetch_url(&self, chat_id: i64, raw: &str) -> Result<String> {
        let mut url: reqwest::Url = raw
            .trim()
            .parse()
            .with_context(|| format!("URL tidak valid: {}", raw.trim()))?;
        check_url_static(&url)?;
        url.set_fragment(None); // fragment tak pernah dikirim HTTP — buang biar bersih
        assert_public_host(&url).await?;

        let mut attempts: Vec<String> = Vec::new();
        let mut content: Option<Fetched> = None;

        macro_rules! try_tier {
            ($name:expr, $fut:expr) => {
                if content.is_none() {
                    match $fut.await {
                        Ok(c) => content = Some(c),
                        Err(e) => {
                            tracing::debug!(url = %url, tier = $name, "fetch tier gagal: {e:#}");
                            attempts.push(format!("{}: {e:#}", $name));
                        }
                    }
                }
            };
        }
        try_tier!("tier-1 direct-markdown", self.tier1_direct(&url));
        try_tier!("tier-2 markdown.new", self.tier2_markdown_new(&url));
        try_tier!("tier-3 r.jina.ai", self.tier3_jina(&url));
        try_tier!("tier-4 html-strip", self.tier4_html(&url));
        let Some(fetched) = content else {
            bail!(
                "❌ Semua tier fetch gagal utk {}:\n{}",
                url,
                attempts.join("\n")
            );
        };

        // Size handling (Pilar 10): > cap → file attachment ke owner, context dapat head.
        let total = fetched.text.chars().count();
        if total > FETCH_MAX_CTX_CHARS {
            let doc = InputFile::memory(fetched.text.clone().into_bytes())
                .file_name(format!("hermes-fetch-{}.md", chrono::Utc::now().timestamp()));
            if let Err(e) = self
                .bot
                .send_document(ChatId(chat_id), doc)
                .caption(format!("📄 Isi penuh {} ({} char, via {})", url, total, fetched.source))
                .await
            {
                tracing::error!(chat_id, "gagal kirim fetch sebagai file: {e:#}");
            }
        }

        tracing::info!(url = %url, via = fetched.source, chars = total, "fetch_url");
        Ok(format!(
            "📄 {} (via {}, {} char):\n{}",
            url,
            fetched.source,
            total,
            head_chars(&fetched.text, FETCH_MAX_CTX_CHARS)
        ))
    }

    /// Tier 1: direct fetch + content negotiation `Accept: text/markdown`.
    /// Situs di balik Cloudflare yang opt-in membalas markdown native — tercepat & gratis.
    /// Non-HTML (markdown/plain/json/xml) langsung diterima apa adanya.
    async fn tier1_direct(&self, url: &reqwest::Url) -> Result<Fetched> {
        let resp = self
            .safe_http
            .get(url.clone())
            .header(
                reqwest::header::ACCEPT,
                "text/markdown, text/plain;q=0.9, application/json;q=0.8, application/xml;q=0.7, text/html;q=0.5",
            )
            .timeout(self.fetch_timeout)
            .send()
            .await?;
        let status = resp.status();
        let ct = content_type(&resp);
        if !status.is_success() {
            bail!("HTTP {status}");
        }
        if ct.contains("html") {
            bail!("balasan HTML ({ct}) — butuh konversi, lanjut tier berikutnya");
        }
        if is_binary_type(&ct) {
            bail!("konten biner ({ct}) tidak didukung fetch_url");
        }
        let body = read_capped(resp).await?;
        if body.trim().is_empty() {
            bail!("body kosong");
        }
        Ok(Fetched { text: body, source: "direct" })
    }

    /// Tier 2: markdown.new (infra Cloudflare; Workers AI + Browser Rendering utk
    /// halaman JS-heavy). Free "always", ±500 req/hari/IP — header rate-limit dilog.
    async fn tier2_markdown_new(&self, url: &reqwest::Url) -> Result<Fetched> {
        let resp = self
            .plain_http
            .get(format!("https://markdown.new/{url}"))
            .timeout(self.fetch_timeout)
            .send()
            .await?;
        let status = resp.status();
        if let Some(rem) = resp
            .headers()
            .get("x-rate-limit-remaining")
            .and_then(|v| v.to_str().ok())
        {
            tracing::info!(remaining = rem, "markdown.new rate limit");
        }
        if !status.is_success() {
            bail!("HTTP {status}");
        }
        let body = read_capped(resp).await?;
        if body.trim().is_empty() {
            bail!("body kosong");
        }
        Ok(Fetched { text: body, source: "markdown.new" })
    }

    /// Tier 3: r.jina.ai keyless — cadangan kalau markdown.new down/limit.
    async fn tier3_jina(&self, url: &reqwest::Url) -> Result<Fetched> {
        let resp = self
            .plain_http
            .get(format!("https://r.jina.ai/{url}"))
            .timeout(self.fetch_timeout)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            bail!("HTTP {status}");
        }
        let body = read_capped(resp).await?;
        if body.trim().is_empty() {
            bail!("body kosong");
        }
        Ok(Fetched { text: body, source: "r.jina.ai" })
    }

    /// Tier 4: plain HTML + parsing lokal (tag-strip naif tanpa dependency readability —
    /// dependency opsional ditambah kalau tier 1-3 terbukti sering gagal).
    async fn tier4_html(&self, url: &reqwest::Url) -> Result<Fetched> {
        let resp = self
            .safe_http
            .get(url.clone())
            .header(reqwest::header::ACCEPT, "text/html")
            .timeout(self.fetch_timeout)
            .send()
            .await?;
        let status = resp.status();
        let ct = content_type(&resp);
        if !status.is_success() {
            bail!("HTTP {status}");
        }
        if !ct.contains("html") {
            bail!("content-type bukan HTML: {ct}");
        }
        let html = read_capped(resp).await?;
        let text = html_to_text(&html);
        if text.trim().is_empty() {
            bail!("hasil parsing kosong");
        }
        Ok(Fetched { text, source: "html-strip" })
    }

    // ── generate_image (Pollinations, Pilar 12) ───────────────────────────────

    pub async fn generate_image(&self, chat_id: i64, prompt: &str) -> Result<String> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("parameter 'prompt' wajib diisi");
        }
        let prompt: String = prompt.chars().take(IMAGE_PROMPT_MAX_CHARS).collect();
        let url = format!("https://image.pollinations.ai/prompt/{}", urlencode(&prompt));

        let resp = self
            .plain_http
            .get(&url)
            .timeout(self.image_timeout)
            .send()
            .await
            .context("gagal memanggil Pollinations (mungkin sedang lambat — coba lagi)")?;
        let status = resp.status();
        if !status.is_success() {
            bail!("Pollinations HTTP {status} — coba lagi atau sederhanakan prompt");
        }
        let ct = content_type(&resp);
        if !ct.starts_with("image/") {
            bail!("Pollinesi membalas {ct}, bukan gambar — coba lagi");
        }
        let bytes = resp.bytes().await.context("gagal membaca bytes gambar")?;
        if bytes.is_empty() {
            bail!("gambar kosong dari Pollinations");
        }
        let kb = bytes.len() / 1024;

        self.bot
            .send_photo(ChatId(chat_id), InputFile::memory(bytes.to_vec()))
            .await
            .context("gagal mengirim foto ke Telegram")?;

        tracing::info!(chat_id, kb, "generate_image terkirim");
        Ok(format!(
            "✅ Gambar terkirim ke owner sebagai foto (≈{kb} KB, 1024×1024, Pollinations). \
             Kamu tidak bisa melihat hasilnya — kalau owner minta revisi (mis. \"birunya \
             kurang\"), panggil lagi dengan prompt yang dimodifikasi sesuai feedback."
        ))
    }
}

// ── Search provider abstraction (Pilar 10 — pola sama seperti AI_PROVIDER) ──────

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>>;
}

pub fn build_search_provider(cfg: &Config) -> Arc<dyn SearchProvider> {
    let tavily = || {
        Arc::new(TavilyProvider {
            http: reqwest::Client::new(),
            api_key: cfg.tavily_api_key.clone(),
        }) as Arc<dyn SearchProvider>
    };
    match cfg.search_provider.as_str() {
        "tavily" => tavily(),
        // Backend lain (brave, google_cse, ddg_scrape) menyusul kalau terbukti perlu.
        other => {
            tracing::warn!(
                "SEARCH_PROVIDER tidak dikenal: {other:?} — fallback ke tavily (satu-satunya backend saat ini)"
            );
            tavily()
        }
    }
}

struct TavilyProvider {
    http: reqwest::Client,
    api_key: Option<String>,
}

#[derive(Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyItem>,
}

#[derive(Deserialize)]
struct TavilyItem {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    fn name(&self) -> &'static str {
        "tavily"
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let Some(key) = self.api_key.as_deref() else {
            bail!(
                "web_search belum dikonfigurasi — set TAVILY_API_KEY di .env \
                 (free tier ±1000 credit/bulan, daftar di tavily.com)"
            );
        };
        let resp = self
            .http
            .post("https://api.tavily.com/search")
            .bearer_auth(key)
            .json(&serde_json::json!({
                "query": query,
                "topic": "general",
                "search_depth": "basic",
                "max_results": max_results,
                "include_answer": false,
                "include_raw_content": false,
            }))
            .timeout(SEARCH_TIMEOUT)
            .send()
            .await
            .context("gagal memanggil Tavily API")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            // 401 = key salah, 432 = quota habis — tampilkan utk debugging owner
            bail!("Tavily HTTP {status}: {}", truncate_chars(&body, 300));
        }
        parse_tavily_body(&body)
    }
}

fn parse_tavily_body(body: &str) -> Result<Vec<SearchResult>> {
    let parsed: TavilyResponse =
        serde_json::from_str(body).context("respons Tavily bukan JSON yang diharapkan")?;
    Ok(parsed
        .results
        .into_iter()
        .map(|r| SearchResult {
            title: r.title.unwrap_or_else(|| "(tanpa judul)".into()),
            url: r.url.unwrap_or_default(),
            snippet: r.content.unwrap_or_default(),
        })
        .collect())
}

fn format_search_results(provider: &str, query: &str, results: &[SearchResult]) -> String {
    let mut out = format!("🔍 \"{query}\" — {} hasil (via {provider}):\n", results.len());
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!(
            "{}. {}\n   {}\n   {}\n",
            i + 1,
            truncate_chars(&r.title, TITLE_MAX_CHARS),
            r.url,
            truncate_chars(&r.snippet, SNIPPET_MAX_CHARS)
        ));
    }
    out.push_str(
        "\nPakai fetch_url kalau butuh isi halaman penuh dari salah satu hasil di atas.",
    );
    out
}

// ── SSRF protection (Pilar 10 hygiene #1) ──────────────────────────────────────

/// Cek statis URL: hanya http/https, tanpa embedded credentials, host harus ada.
fn check_url_static(url: &reqwest::Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("hanya URL http/https yang didukung (dapat: {})", url.scheme());
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("URL dengan embedded credentials tidak didukung (Pilar 10 hygiene #4)");
    }
    if url.host_str().is_none() {
        bail!("URL tanpa hostname");
    }
    Ok(())
}

/// Validasi awal sebelum chain fetch: IP literal dicek langsung; hostname di-resolve
/// dan SEMUA IP harus publik (err ke sisi aman). Ini terutama utk tier 2/3 yang
/// menyerahkan URL ke pihak ketiga — jangan pernah bocorkan hostname internal.
async fn assert_public_host(url: &reqwest::Url) -> Result<()> {
    let host = url.host_str().expect("checked di check_url_static").to_string();
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            bail!("SSRF guard: {host} adalah IP private/reserved — ditolak");
        }
        return Ok(());
    }
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
        .await
        .with_context(|| format!("gagal resolve hostname {host}"))?
        .collect();
    if addrs.is_empty() {
        bail!("hostname {host} tidak resolve ke alamat mana pun");
    }
    if let Some(blocked) = addrs.iter().map(|a| a.ip()).find(|ip| is_blocked_ip(*ip)) {
        bail!("SSRF guard: {host} resolve sebagian ke IP private/reserved ({blocked}) — ditolak");
    }
    Ok(())
}

/// Apakah IP masuk rentang private/reserved yang dilarang.
/// Daftar: loopback, unspecified, private RFC1918, link-local (termasuk cloud metadata
/// 169.254.169.254), CGNAT 100.64/10, IETF 192.0.0/24, benchmark 198.18/15,
/// multicast/reserved v4 (>=224), ULA fc00::/7, link-local & multicast v6.
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_link_local() // 169.254.0.0/16 — termasuk metadata 169.254.169.254
                || o[0] == 10
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // benchmarking
                || (o[0] == 100 && (64..=127).contains(&o[1])) // CGNAT
                || o[0] >= 224 // multicast + reserved 240/4 + broadcast
        }
        IpAddr::V6(v6) => {
            // IPv4-mapped (::ffff:a.b.c.d) → cek sebagai IPv4
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            v6.is_loopback() // ::1
                || v6.is_unspecified() // ::
                || (s[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
                || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || (s[0] & 0xff00) == 0xff00 // multicast ff00::/8
        }
    }
}

/// DNS resolver SSRF-safe untuk reqwest: resolve → tolak kalau SEMUA IP private →
/// saring sehingga hanya IP publik yang pernah sampai ke layer connect.
/// Dipasang di `safe_http` (tier 1 & 4); tiap koneksi — termasuk redirect — lewat sini,
/// jadi redirect ke IP private otomatis gagal resolve.
#[derive(Clone, Default)]
struct SafeResolver;

impl reqwest::dns::Resolve for SafeResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            // IP literal (termasuk bentuk [::1] tanpa bracket di Name)
            if let Ok(ip) = host.parse::<IpAddr>() {
                if is_blocked_ip(ip) {
                    return Err(anyhow!("SSRF guard: {host} IP private/reserved").into());
                }
                let addrs = vec![SocketAddr::new(ip, 0)];
                return Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs);
            }
            let safe: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| anyhow!("resolve {host} gagal: {e}"))?
                .filter(|a| !is_blocked_ip(a.ip()))
                .collect();
            if safe.is_empty() {
                return Err(anyhow!("SSRF guard: {host} hanya resolve ke IP private/reserved").into());
            }
            Ok(Box::new(safe.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

struct Fetched {
    text: String,
    source: &'static str,
}

fn content_type(resp: &reqwest::Response) -> String {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn is_binary_type(ct: &str) -> bool {
    ct.starts_with("image/")
        || ct.starts_with("video/")
        || ct.starts_with("audio/")
        || ct.contains("pdf")
        || ct.contains("zip")
        || ct.contains("octet-stream")
}

/// Baca body dengan batas ukuran — bail sebelum melebihi MAX_DOWNLOAD_BYTES.
async fn read_capped(resp: reqwest::Response) -> Result<String> {
    let mut resp = resp;
    let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
    while let Some(chunk) = resp.chunk().await.context("gagal membaca body")? {
        buf.extend_from_slice(&chunk);
        if buf.len() > MAX_DOWNLOAD_BYTES {
            bail!(
                "konten > {} MB — dibatalkan (size cap Pilar 10)",
                MAX_DOWNLOAD_BYTES / 1024 / 1024
            );
        }
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn head_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    if total <= n {
        return s.to_string();
    }
    let end = s
        .char_indices()
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!(
        "{}…(dipotong — {total} char total; versi penuh dikirim sebagai file ke owner)",
        &s[..end]
    )
}

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Percent-encoding sederhana (unreserved + %XX) untuk prompt image di URL.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Cari `needle` di `haystack` mulai byte `from`, case-insensitive ASCII (1:1 byte →
/// indeks tetap valid untuk string asli). Return posisi byte di string asli.
fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if from >= haystack.len() || needle.is_empty() {
        return None;
    }
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    let mut i = from;
    while i + n.len() <= h.len() {
        if h[i..i + n.len()]
            .iter()
            .zip(n)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Buang blok berpasangan `<tag ...> ... </tag>` (script/style/noscript/svg) + komentar HTML.
fn remove_html_blocks(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        if let Some(s) = find_ci(html, &open, i) {
            out.push_str(&html[i..s]);
            // pastikan benar-benar tag (bukan "<scriptx")
            let after = html.as_bytes().get(s + open.len());
            let is_tag = match after {
                Some(b) => matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/'),
                None => true,
            };
            if is_tag {
                if let Some(c) = find_ci(html, &close, s) {
                    let end = html[c..].find('>').map(|g| c + g + 1).unwrap_or(html.len());
                    i = end;
                    continue;
                }
            }
            out.push_str(&html[s..s + open.len()]);
            i = s + open.len();
        } else {
            out.push_str(&html[i..]);
            break;
        }
    }
    out
}

/// HTML → teks naif (tier 4, tanpa dependency readability): ekstrak <title>, buang
/// script/style/comment/head, tag → newline utk block element, strip semua tag,
/// decode entity umum, rapikan whitespace.
fn html_to_text(html: &str) -> String {
    let title = find_ci(html, "<title", 0)
        .and_then(|s| html[s..].find('>').map(|g| s + g + 1))
        .and_then(|start| find_ci(html, "</title", start).map(|end| html[start..end].to_string()))
        .map(|t| format!("# {}\n\n", decode_entities(t.trim())) )
        .unwrap_or_default();

    let mut body = remove_html_blocks(html, "script");
    body = remove_html_blocks(&body, "style");
    body = remove_html_blocks(&body, "noscript");
    body = remove_html_blocks(&body, "svg");
    if let Some(hs) = find_ci(&body, "<head", 0) {
        if let Some(he) = find_ci(&body, "</head", hs) {
            let end = body[he..].find('>').map(|g| he + g + 1).unwrap_or(body.len());
            body.replace_range(hs..end, "");
        }
    }

    // strip komentar
    let mut text = String::with_capacity(body.len());
    let mut rest: &str = &body;
    while let Some(s) = rest.find("<!") {
        let after = &rest[s..];
        if after.starts_with("<!--") {
            text.push_str(&rest[..s]);
            match rest[s..].find("-->") {
                Some(e) => rest = &rest[s + e + 3..],
                None => {
                    rest = "";
                    break;
                }
            }
        } else {
            // doctype dsb — buang sampai '>'
            text.push_str(&rest[..s]);
            match rest[s..].find('>') {
                Some(e) => rest = &rest[s + e + 1..],
                None => {
                    rest = "";
                    break;
                }
            }
        }
    }
    text.push_str(rest);

    // strip semua tag; tag block/br → newline
    let mut out = title;
    let mut rest: &str = &text;
    while let Some(s) = rest.find('<') {
        out.push_str(&rest[..s]);
        let Some(g) = rest[s..].find('>') else { break };
        let tag = rest[s + 1..s + g].trim();
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "br" | "p" | "div" | "li" | "tr" | "blockquote" | "pre" | "section" | "article"
                | "header" | "footer" | "ul" | "ol" | "table" | "h1" | "h2" | "h3" | "h4"
                | "h5" | "h6"
        ) {
            out.push('\n');
        }
        rest = &rest[s + g + 1..];
    }
    out.push_str(rest);

    // decode entities + rapikan whitespace
    let decoded = decode_entities(&out);
    let mut clean = String::with_capacity(decoded.len());
    let mut blank = 0;
    for line in decoded.lines() {
        let l = line.trim();
        if l.is_empty() {
            blank += 1;
            if blank <= 1 {
                clean.push('\n');
            }
        } else {
            blank = 0;
            clean.push_str(l);
            clean.push('\n');
        }
    }
    clean.trim().to_string()
}

/// Decode entity HTML umum (named + numeric dec/hex).
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(a) = rest.find('&') {
        out.push_str(&rest[..a]);
        let after = &rest[a..];
        let Some(sc) = after.find(';') else {
            out.push('&');
            rest = &rest[a + 1..];
            continue;
        };
        if sc > 12 || sc < 3 {
            out.push('&');
            rest = &rest[a + 1..];
            continue;
        }
        let ent = &after[1..sc];
        let repl = match ent {
            "amp" => Some('&'.to_string()),
            "lt" => Some('<'.to_string()),
            "gt" => Some('>'.to_string()),
            "quot" => Some('"'.to_string()),
            "apos" | "#39" => Some('\''.to_string()),
            "nbsp" => Some(' '.to_string()),
            "mdash" => Some("—".into()),
            "ndash" => Some("–".into()),
            "hellip" => Some("…".into()),
            "laquo" => Some("«".into()),
            "raquo" => Some("»".into()),
            "copy" => Some("©".into()),
            "reg" => Some("®".into()),
            "trade" => Some("™".into()),
            _ => {
                if let Some(num) = ent.strip_prefix('#') {
                    let cp = if let Some(hex) = num.strip_prefix(['x', 'X']) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        num.parse::<u32>().ok()
                    };
                    cp.and_then(char::from_u32).map(|c| c.to_string())
                } else {
                    None
                }
            }
        };
        match repl {
            Some(r) => {
                out.push_str(&r);
                rest = &after[sc + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[a + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_blocking() {
        let blocked = [
            "127.0.0.1", "127.255.255.255", "10.0.0.1", "10.255.1.1", "172.16.0.1",
            "172.31.255.255", "192.168.1.1", "169.254.169.254", "169.254.0.1", "0.0.0.0",
            "100.64.0.1", "100.127.255.255", "192.0.0.8", "198.18.0.5", "198.19.5.5",
            "224.0.0.1", "240.0.0.1", "255.255.255.255", "::1", "::", "fc00::1", "fd12::1",
            "fe80::1", "ff02::1", "::ffff:127.0.0.1", "::ffff:10.0.0.5",
            "::ffff:192.168.0.1",
        ];
        for ip in blocked {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(is_blocked_ip(ip), "{ip} seharusnya diblokir");
        }
        let allowed = [
            "1.1.1.1", "8.8.8.8", "172.32.0.1", "172.15.0.1", "100.128.0.1", "100.63.0.1",
            "203.0.113.5", "2606:4700:4700::1111", "2001:db8::1", "2a00:1450:4001::81",
        ];
        for ip in allowed {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(!is_blocked_ip(ip), "{ip} seharusnya lolos");
        }
    }

    #[test]
    fn url_static_validation() {
        for raw in [
            "ftp://example.com/file",
            "file:///etc/passwd",
            "http://user:pass@example.com/",
            "https://127.0.0.1:8080/admin",
            "https://[::1]/x",
            "https://10.0.0.5/",
            "https://169.254.169.254/latest/meta-data",
        ] {
            let url: reqwest::Url = raw.parse().unwrap();
            // IP private tertangkap statis; sisanya (scheme/credentials) juga
            let res = check_url_static(&url).and_then(|_| {
                let host = url.host_str().unwrap();
                if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
                    if is_blocked_ip(ip) {
                        bail!("blocked");
                    }
                }
                Ok(())
            });
            assert!(res.is_err(), "{raw} seharusnya ditolak");
        }
        for raw in ["https://example.com/page?q=1", "http://example.org"] {
            let url: reqwest::Url = raw.parse().unwrap();
            check_url_static(&url).expect("URL publik valid seharusnya lolos");
        }
    }

    #[test]
    fn html_stripping() {
        let html = "<html><head><title>Judul Halaman</title><style>body{color:red}</style>\
</head><body><h1>Selamat&nbsp;datang</h1><script>alert('x')</script>\
<p>Paragraf <b>tebal</b> &amp; italic.</p><!-- komentar --><br/>Baris baru\
<footer>foot</footer></body></html>";
        let text = html_to_text(html);
        assert!(text.starts_with("# Judul Halaman"), "title diekstrak: {text}");
        assert!(text.contains("Selamat datang"), "nbsp didecode: {text}");
        assert!(text.contains("tebal & italic"), "entity amp + tag strip: {text}");
        assert!(text.contains("Baris baru"), "br jadi newline efektif: {text}");
        assert!(!text.contains("alert"), "script dibuang: {text}");
        assert!(!text.contains("color:red"), "style dibuang: {text}");
        assert!(!text.contains("komentar"), "komentar dibuang: {text}");
        assert!(!text.contains('<'), "tanpa tag tersisa: {text}");
    }

    #[test]
    fn entity_decoding() {
        assert_eq!(decode_entities("a &amp; b &lt;x&gt; &#65;&#x42; &unknown;"), "a & b <x> AB &unknown;");
        assert_eq!(decode_entities("100&nbsp;% &mdash; ok"), "100 % — ok");
    }

    #[test]
    fn case_insensitive_search() {
        let s = "Hello <SCRIPT type=x> World </script> end";
        assert_eq!(find_ci(s, "<script", 0), Some(6));
        assert_eq!(find_ci(s, "</SCRIPT", 0), Some(28));
        assert_eq!(find_ci(s, "WORLD", 0), Some(22));
        assert_eq!(find_ci(s, "xyz", 0), None);
        assert_eq!(find_ci(s, "script", 40), None);
    }

    #[test]
    fn script_block_removal_case_insensitive() {
        let html = "a<ScRiPt>evil()</sCrIpT>b";
        assert_eq!(remove_html_blocks(html, "script"), "ab");
        // tag serupa tapi bukan script — tidak terbuang
        assert_eq!(remove_html_blocks("a<scriptx>b</scriptx>c", "script"), "a<scriptx>b</scriptx>c");
    }

    #[test]
    fn tavily_body_parsing() {
        let body = r#"{"query":"rust","results":[
            {"title":"Rust Lang","url":"https://rust-lang.org","content":"Bahasa sistem <b>aman</b>","score":0.99},
            {"url":"https://example.org/x","content":"Tanpa judul"}
        ]}"#;
        let res = parse_tavily_body(body).unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].title, "Rust Lang");
        assert_eq!(res[1].title, "(tanpa judul)");
        assert!(parse_tavily_body("bukan json").is_err());
        assert_eq!(parse_tavily_body(r#"{"results":[]}"#).unwrap().len(), 0);
    }

    #[test]
    fn search_result_formatting_caps() {
        let long: String = "x".repeat(1000);
        let results = vec![
            SearchResult {
                title: long.clone(),
                url: "https://example.com".into(),
                snippet: long.clone(),
            };
            2
        ];
        let out = format_search_results("tavily", "query", &results);
        assert!(out.contains("2 hasil"));
        assert!(out.chars().count() < 2500, "cap snippet/judul menjaga context ramping");
    }

    #[test]
    fn url_encoding() {
        assert_eq!(urlencode("hello world"), "hello%20world");
        assert_eq!(urlencode("a+b&c=d/é"), "a%2Bb%26c%3Dd%2F%C3%A9");
        assert_eq!(urlencode("safe.-_~"), "safe.-_~");
    }

    #[test]
    fn head_chars_truncation() {
        let s = "y".repeat(20_000);
        let out = head_chars(&s, FETCH_MAX_CTX_CHARS);
        assert!(out.starts_with('y'));
        assert!(out.contains("dipotong"));
    }

    // ── Integration test (network) — jalankan manual: cargo test -- --ignored ──

    fn test_ctx() -> Arc<WebCtx> {
        use std::path::PathBuf;
        let cfg = crate::config::Config {
            telegram_bot_token: "1:test".into(),
            database_url: "postgres://x".into(),
            anthropic_api_key: "x".into(),
            anthropic_model: "m".into(),
            anthropic_base_url: "https://x".into(),
            ai_provider: "anthropic".into(),
            allowed_chat_ids: vec![123],
            n_context: 20,
            soul_path: "soul.md".into(),
            work_roots: vec![PathBuf::from(".")],
            run_cmd_timeout: 120,
            confirm_timeout: 300,
            search_provider: "tavily".into(),
            tavily_api_key: None,
            fetch_timeout: 30,
            image_timeout: 60,
            skills_dir: "skills".into(),
            ocr_lang: "eng".into(),
            ocr_tessdata: None,
        };
        // tidak butuh token valid — Bot::new tak melakukan network call
        WebCtx::new(&cfg, teloxide::Bot::new("1:test"))
    }

    /// SSRF end-to-end: `localtest.me` adalah wildcard DNS publik yang selalu resolve
    /// ke 127.0.0.1 — guard harus menolaknya (baik via pre-check maupun SafeResolver).
    #[tokio::test]
    #[ignore = "butuh network"]
    async fn ssrf_guard_rejects_dns_to_loopback() {
        let ctx = test_ctx();
        for url in [
            "http://localtest.me/secret",
            "http://127.0.0.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.1:8080/admin",
        ] {
            let res = ctx.fetch_url(123, url).await;
            assert!(res.is_err(), "{url} seharusnya ditolak SSRF guard");
            let msg = format!("{:#}", res.unwrap_err());
            assert!(msg.contains("SSRF guard"), "{url}: pesan = {msg}");
        }
    }

    /// Fetch chain live: example.com balas HTML → tier-1 gagal → tier-2 markdown.new
    /// harus berhasil konversi jadi markdown.
    #[tokio::test]
    #[ignore = "butuh network"]
    async fn fetch_real_url_via_chain() {
        let ctx = test_ctx();
        let out = ctx.fetch_url(123, "https://example.com").await.expect("fetch harus sukses");
        assert!(out.contains("via "), "{out}");
        assert!(out.contains("Example Domain"), "konten utama terekstrak: {out}");
    }
}
