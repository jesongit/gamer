//! 认证与会话治理（阶段 2 SEC-001/002/003）
//!
//! 结构一览：
//! - [`AuthState`]：内存会话表 + 登录限流表 + 凭据校验。**重启即全体失效**是
//!   设计行为（会话无持久化价值，重新登录成本极低）。
//! - [`auth_guard`]：axum 中间件，保护 build_router 里"受保护分组"的全部路由
//!   （其余 /api/**、/ws/device/:id）。豁免清单见 api/mod.rs 的 public 分组
//!   （login / session / logout / health / 静态资源）。
//! - 回环管理通道：带 `X-Admin-Token` 的请求仅在「来源 IP 为回环 && token 匹配」
//!   时视为已认证——专供本机管理脚本（gamer.ps1 stop 优雅停机）。token 来源：
//!   环境变量 GAMER_ADMIN_TOKEN；dev 缺省时启动自动生成，仅以 WARN 提示通道启用，
//!   不打印令牌值；prod 不设置则通道直接禁用。
//! - 同源防护：Cookie SameSite=Strict 是 CSRF 主防线；此处再拦一层 Origin/Host
//!   不一致的状态变更请求（POST/PUT/DELETE/PATCH 与 WS 升级），Origin 缺失放行
//!   （CLI/curl 场景）。
//!
//! 防护形态标注——高风险接口（均有专项测试覆盖）：
//!   shutdown / 设备控制 / 脚本运行·停止 / 模板删除 / ZIP 导入
//! 全部位于受保护分组：未登录 401；shutdown 另有回环 token 快捷通道；
//! ZIP 导入另有资源硬限（见 scripts.rs import）。

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, header::HeaderMap, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::RngCore;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tracing::{debug, warn};

use crate::config::AuthConfig;

/// 会话 Cookie 名（前端契约钉死值）
pub const SESSION_COOKIE: &str = "gb_session";
/// 回环管理通道的令牌头
pub const ADMIN_TOKEN_HEADER: &str = "X-Admin-Token";
/// 过期会话后台清扫周期（每小时一次）
const SWEEP_PERIOD: Duration = Duration::from_secs(3600);
/// 开发模式自动生成回环管理凭据时的非敏感提示；不得附带凭据名或凭据值。
const DEV_ADMIN_TOKEN_NOTICE: &str =
    "dev 模式已自动生成本机管理凭据（仅回环可用）；如需脚本复用请设置环境变量";
/// 登录限流表容量上限：达到后整表收缩（防大量用户名组合灌爆内存的最后兜底；
/// 实际来源 IP 无法伪造 socket 地址，正常单机场景远远用不到这个量级）
const MAX_TRACKED_LOGIN_KEYS: usize = 10_000;

/// 登录凭据快照（构造 AuthState 时解析定死，运行期不变）
#[derive(Debug, Clone)]
pub enum Credential {
    /// 环境变量 GAMER_ADMIN_PASSWORD（优先级最高）
    EnvPassword(String),
    /// 环境变量 GAMER_ADMIN_PASSWORD_FILE 指向的密钥文件
    SecretFile(String),
    /// config [auth].password_hash = sha256$salt$hex（仅兼容旧配置，成功登录后升级）
    Hash { salt: Vec<u8>, digest: Vec<u8> },
    /// Argon2id PHC 哈希（推荐配置格式）
    Argon2(String),
    /// 开发模式内置默认值；成功登录后仅在内存中升级到 Argon2id
    Plain(String),
    /// 凭据配置非法或生产模式没有强凭据时的 fail-closed 状态。
    /// 不携带原始配置内容，避免误把口令/哈希带入调试输出。
    Unavailable,
}

/// 解析 Argon2id PHC 或旧版 `sha256$salt$hex` 口令哈希格式。
///
/// 旧格式只为迁移期保留，不能作为新配置生成格式；Argon2 参数和摘要格式
/// 交由 password-hash 解析器校验，避免自行重新实现 PHC 语法。
pub fn parse_password_hash(s: &str) -> Result<Credential, String> {
    let trimmed = s.trim();
    if trimmed.starts_with("$argon2id$") {
        validate_argon2_phc(trimmed)?;
        return Ok(Credential::Argon2(trimmed.to_string()));
    }

    parse_legacy_sha256_hash(trimmed)
}

fn validate_argon2_phc(s: &str) -> Result<(), String> {
    let parts: Vec<&str> = s.split('$').collect();
    if parts.len() != 6 || !parts[0].is_empty() || parts[1] != "argon2id" {
        return Err("argon2id PHC 哈希段数或算法非法".into());
    }
    if parts[2] != "v=19" {
        return Err("argon2id 仅支持 v=19".into());
    }
    let mut memory_kib = None;
    let mut iterations = None;
    let mut lanes = None;
    for parameter in parts[3].split(',') {
        let (name, value) = parameter
            .split_once('=')
            .ok_or_else(|| "argon2id 参数格式非法".to_string())?;
        let value = value
            .parse::<u32>()
            .map_err(|_| "argon2id 参数值非法".to_string())?;
        match name {
            "m" if memory_kib.is_none() => memory_kib = Some(value),
            "t" if iterations.is_none() => iterations = Some(value),
            "p" if lanes.is_none() => lanes = Some(value),
            _ => return Err("argon2id 参数集合非法".into()),
        }
    }
    let memory_kib = memory_kib.ok_or_else(|| "argon2id 缺少 m 参数".to_string())?;
    let iterations = iterations.ok_or_else(|| "argon2id 缺少 t 参数".to_string())?;
    let lanes = lanes.ok_or_else(|| "argon2id 缺少 p 参数".to_string())?;
    if !(8 * 1024..=1024 * 1024).contains(&memory_kib)
        || !(1..=10).contains(&iterations)
        || !(1..=16).contains(&lanes)
        || parts[4].is_empty()
        || parts[5].is_empty()
    {
        return Err("argon2id 参数超出安全范围或摘要为空".into());
    }
    PasswordHash::new(s).map_err(|_| "argon2id PHC 哈希编码非法".to_string())?;
    Ok(())
}

/// 解析仅用于兼容的 `sha256$salt$hex` 口令哈希格式。
/// salt：hex 编码、≥8 字节（16 hex 字符）；digest：sha256 摘要 32 字节的 hex（64 字符）。
fn parse_legacy_sha256_hash(s: &str) -> Result<Credential, String> {
    let parts: Vec<&str> = s.trim().split('$').collect();
    if parts.len() != 3 || parts[0] != "sha256" {
        return Err(format!("段数 {} 不符或算法前缀不是 sha256", parts.len()));
    }
    let salt = decode_hex(parts[1]).map_err(|_| "salt 不是合法 hex".to_string())?;
    if salt.len() < 8 {
        return Err(format!("salt 仅 {} 字节，要求 ≥8", salt.len()));
    }
    let digest = decode_hex(parts[2]).map_err(|_| "digest 不是合法 hex".to_string())?;
    if digest.len() != 32 {
        return Err(format!("digest 长 {} 字节，sha256 应为 32", digest.len()));
    }
    Ok(Credential::Hash { salt, digest })
}

/// 生成可直接放入 `[auth].password_hash` 的 Argon2id PHC 哈希。
///
/// 调用方只能得到不可逆哈希；密码不会写日志，也不会由本模块落盘。
pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| "argon2id 口令哈希生成失败".to_string())
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("奇数长度 hex".into());
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let s = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
            u8::from_str_radix(s, 16).map_err(|e| e.to_string())
        })
        .collect()
}

/// 常量时间字节比较：先走完等长 xor 再下结论；长度不等也耗同等工作量
/// （两侧均为定长摘要或本机已知串，长度不构成可利用的时序侧信道，防御性抹平）
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut acc = 0u8;
    for (lhs, rhs) in a.iter().copied().zip(b.iter().copied()) {
        acc |= lhs ^ rhs;
    }
    if a.len() != b.len() {
        acc |= 1;
    }
    let _ = std::hint::black_box(acc);
    // 长度不等时 acc 必然非零吗？不一定——用长度谓词兜底收尾
    a.len() == b.len() && acc == 0
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// 系统 ≥128bit 随机十六进制 ID（会话与 dev 管理令牌共用；这里给 256bit）
fn random_hex_id(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

struct Session {
    username: String,
    /// 绝对过期时刻（登录起算，续期不可延长）
    abs_expire: Instant,
    /// 最近一次认证请求时刻（滑动空闲判据）
    last_seen: Instant,
}

#[derive(Default)]
struct LoginFails {
    attempts: VecDeque<Instant>,
}

/// 登录失败桶使用结构化 `(来源 IP, 用户名)` 键，避免字符串拼接歧义，也避免
/// 同一来源对无关用户名的失败尝试误锁唯一管理员账号。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct LoginKey {
    ip: String,
    username: String,
}

#[derive(Debug)]
pub enum LoginError {
    /// 用户名或口令错误 → 401 invalid_credentials
    Invalid,
    /// 触发限流 → 429 too_many_attempts（携带建议重试秒数）
    RateLimited { retry_after_secs: u64 },
}

pub struct AuthState {
    inner: Mutex<Inner>,
    credential: RwLock<Credential>,
    secure_cookies: bool,
    /// 回环管理令牌（None = 通道禁用）
    admin_token: Option<String>,
    cfg: AuthConfig,
}

struct Inner {
    sessions: HashMap<String, Session>,
    fails: HashMap<LoginKey, LoginFails>,
}

impl AuthState {
    /// 组装鉴权状态。credential 由 main 在配置加载后按
    /// GAMER_ADMIN_PASSWORD > GAMER_ADMIN_PASSWORD_FILE > [auth].password_hash >
    /// 开发默认值链路解析传入。
    pub fn new(
        credential: Credential,
        cfg: AuthConfig,
        secure_cookies: bool,
        admin_token: Option<String>,
    ) -> Self {
        Self {
            inner: Mutex::new(Inner {
                sessions: HashMap::new(),
                fails: HashMap::new(),
            }),
            credential: RwLock::new(credential),
            secure_cookies,
            admin_token,
            cfg,
        }
    }

    /// 当前生效的凭据来源描述（启动摘要日志用，绝不含凭据内容）
    pub fn credential_source(&self) -> &'static str {
        match &*self.credential.read().unwrap() {
            Credential::EnvPassword(_) => "env:GAMER_ADMIN_PASSWORD",
            Credential::SecretFile(_) => "env:GAMER_ADMIN_PASSWORD_FILE",
            Credential::Hash { .. } | Credential::Argon2(_) => "config:password_hash",
            Credential::Plain(_) => "dev:built-in-default",
            Credential::Unavailable => "unavailable:invalid-or-missing-credential",
        }
    }

    pub fn secure_cookies(&self) -> bool {
        self.secure_cookies
    }

    /// 凭据核验：新配置使用 Argon2id；旧格式仅作为迁移兼容路径。
    pub(crate) fn verify_credentials(&self, input: &str) -> bool {
        match &*self.credential.read().unwrap() {
            Credential::Argon2(encoded) => {
                let Ok(parsed) = PasswordHash::new(encoded) else {
                    return false;
                };
                Argon2::default()
                    .verify_password(input.as_bytes(), &parsed)
                    .is_ok()
            }
            Credential::EnvPassword(p) | Credential::SecretFile(p) | Credential::Plain(p) => {
                // 迁移兼容：不再作为新配置格式；成功登录后会替换为 Argon2id。
                ct_eq(&sha256(input.as_bytes()), &sha256(p.as_bytes()))
            }
            Credential::Hash { salt, digest } => {
                let mut m = Sha256::new();
                m.update(salt);
                m.update(input.as_bytes());
                ct_eq(&m.finalize(), digest)
            }
            Credential::Unavailable => false,
        }
    }

    /// 登录尝试：成功返回 (session_id, username)，失败给出契约错误分类。
    /// 限流键为 `(来源 IP, 用户名)` 组合；IP 取不到时为 "unknown"，相同用户名的
    /// 非标准直连共享桶。用户名不做折叠，因为当前唯一合法值精确为 `admin`。
    pub fn attempt_login(
        &self,
        username: &str,
        password: &str,
        ip_key: &str,
    ) -> Result<(String, String), LoginError> {
        // 形状粗校验先于限流判定？不：先查限流（被封锁期间连形状探测也不做），
        // 但形状不合格计一次失败（组合爆破面收敛到同一桶）
        let now = Instant::now();
        let mut g = self.inner.lock().unwrap();
        self.prune_fails(&mut g.fails, now);
        let fail_key = LoginKey {
            ip: ip_key.to_string(),
            username: username.to_string(),
        };

        if let Some(f) = g.fails.get(&fail_key) {
            if f.attempts.len() >= self.cfg.login_max_fails as usize {
                let oldest = f.attempts.front().copied().unwrap_or(now);
                let retry = self
                    .window_duration()
                    .checked_sub(now.duration_since(oldest))
                    .map(|d| d.as_secs().max(1))
                    .unwrap_or(1);
                return Err(LoginError::RateLimited {
                    retry_after_secs: retry,
                });
            }
        }

        // 用户名固定单管理员；密码链路见 credential_source
        let cred_ok = username == "admin"
            && !username.is_empty()
            && !password.is_empty()
            && password.len() <= 1024
            && self.verify_credentials(password);
        if !cred_ok {
            let entry = g.fails.entry(fail_key.clone()).or_default();
            entry.attempts.push_back(now);
            while entry.attempts.len() > self.cfg.login_max_fails as usize {
                entry.attempts.pop_front();
            }
            if g.fails.len() > MAX_TRACKED_LOGIN_KEYS {
                warn!(
                    keys = g.fails.len(),
                    "login-fail table over capacity, pruning all entries"
                );
                g.fails.clear(); // 极端灌压场景整体失忆，优先保活进程
            }
            return Err(LoginError::Invalid);
        }

        // 旧明文/旧 SHA-256 只在成功认证后于内存中升级；不回写配置文件，
        // 不记录 password，也不把 password 带入错误信息。下次进程启动时仍
        // 可读取旧配置，直到管理员将新 PHC 哈希写入 password_hash。
        self.upgrade_legacy_credential(password);
        g.fails.remove(&fail_key); // 成功即清空该 IP+用户名组合的失败计数
        let sid = random_hex_id(32); // 256bit 高熵 ID
        g.sessions.insert(
            sid.clone(),
            Session {
                username: "admin".to_string(),
                abs_expire: now + self.abs_duration(),
                last_seen: now,
            },
        );
        Ok((sid, "admin".to_string()))
    }

    fn upgrade_legacy_credential(&self, password: &str) {
        let Ok(encoded) = hash_password(password) else {
            return;
        };
        let mut credential = self.credential.write().unwrap();
        if matches!(
            &*credential,
            Credential::EnvPassword(_) | Credential::Hash { .. } | Credential::Plain(_)
        ) {
            *credential = Credential::Argon2(encoded);
        }
    }

    /// 校验并滑动续期：命中返回用户名；绝对/空闲到期均即时销毁并拒绝
    pub fn validate(&self, sid: &str) -> Option<String> {
        let now = Instant::now();
        let mut g = self.inner.lock().unwrap();
        let s = g.sessions.get_mut(sid)?;
        if now >= s.abs_expire {
            g.sessions.remove(sid);
            return None;
        }
        if now.duration_since(s.last_seen) >= self.idle_duration() {
            g.sessions.remove(sid);
            return None;
        }
        s.last_seen = now;
        Some(s.username.clone())
    }

    /// 销毁指定会话（登出/接管失效）；幂等
    pub fn destroy(&self, sid: &str) {
        self.inner.lock().unwrap().sessions.remove(sid);
    }

    /// 后台清扫：清两类过期（绝对到期 / 空闲超时）。build_router 启动小时级循环任务调用
    pub fn sweep(&self) {
        let now = Instant::now();
        let mut g = self.inner.lock().unwrap();
        let idle = self.idle_duration();
        g.sessions
            .retain(|_, s| now < s.abs_expire && now.duration_since(s.last_seen) < idle);
        self.prune_fails(&mut g.fails, now);
    }

    #[cfg(test)] // 仅测试透出：断言会话表规模
    pub fn sessions_len(&self) -> usize {
        self.inner.lock().unwrap().sessions.len()
    }

    fn abs_duration(&self) -> Duration {
        Duration::from_secs(self.cfg.session_abs_secs.max(1))
    }

    fn idle_duration(&self) -> Duration {
        Duration::from_secs(self.cfg.session_idle_secs.max(1))
    }

    fn window_duration(&self) -> Duration {
        Duration::from_secs(self.cfg.login_window_secs.max(1))
    }

    fn prune_fails(&self, fails: &mut HashMap<LoginKey, LoginFails>, now: Instant) {
        let window = self.window_duration();
        fails.retain(|_, f| {
            f.attempts.retain(|t| now.duration_since(*t) < window);
            !f.attempts.is_empty()
        });
    }

    // ---------- Cookie ----------

    pub fn session_cookie_for(&self, sid: &str) -> String {
        format!(
            "{}={}; Path=/; HttpOnly; SameSite=Strict{}",
            SESSION_COOKIE,
            sid,
            if self.secure_cookies { "; Secure" } else { "" }
        )
    }

    pub fn expired_cookie(&self) -> String {
        format!(
            "{}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{}",
            SESSION_COOKIE,
            if self.secure_cookies { "; Secure" } else { "" }
        )
    }

    /// 从 Cookie 头提取会话 id（仅认完全匹配的名值对；空值视为缺失）
    pub fn extract_sid(headers: &HeaderMap) -> Option<String> {
        let raw = headers.get(header::COOKIE)?.to_str().ok()?;
        raw.split(';').find_map(|pair| {
            pair.trim()
                .strip_prefix(&format!("{SESSION_COOKIE}=")) // 每对形如 name=value
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        })
    }

    // ---------- 回环管理通道 ----------

    /// 来源 IP 为回环且 X-Admin-Token 与服务端令牌常量时间相等才放行。
    /// ConnectInfo 缺失（非常规路径直接构造的请求）按非回环拒绝。
    pub fn admin_token_admits(
        &self,
        remote: Option<SocketAddr>,
        header_token: Option<&str>,
    ) -> bool {
        let Some(token) = &self.admin_token else {
            return false;
        };
        let Some(addr) = remote else {
            return false;
        };
        if !addr.ip().is_loopback() {
            return false;
        }
        let Some(got) = header_token.map(str::trim).filter(|s| !s.is_empty()) else {
            return false;
        };
        ct_eq(got.as_bytes(), token.as_bytes())
    }
}

// ---------- Origin/Host 同源防护（纯函数便于穷举测试） ----------

/// Origin 存在时其 authority 必须与 Host 一致；Origin 缺失放行（CLI/curl 场景）。
/// 有 Origin 却没有 Host 的畸形请求一律拒绝。"null" Origin 自然落入不一致分支。
pub fn origin_allows(origin: Option<&str>, host: Option<&str>) -> bool {
    let Some(origin) = origin.map(str::trim).filter(|o| !o.is_empty()) else {
        return true;
    };
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return false;
    };
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    !authority.is_empty()
        && !authority.contains('@')
        && !authority.chars().any(char::is_whitespace)
        && authority.eq_ignore_ascii_case(host)
}

fn is_state_changing(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::DELETE | Method::PATCH
    )
}

// ---------- 中间件 ----------

/// 受保护分组的统一门卫。决策顺序：
/// 1. 回环管理通道快捷放行（本机管理脚本无 Cookie 可携）；
/// 2. 状态变更方法 / WS 升级做 Origin↔Host 一致性检查，违规 403；
/// 3. Cookie 会话校验+续期，未通过统一 401 {"error":"unauthorized"}。
pub async fn auth_guard(
    State(auth): State<std::sync::Arc<AuthState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let remote = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    match admit(&auth, req.method(), req.headers(), remote) {
        Decision::Admit => next.run(req).await,
        Decision::Forbidden => {
            debug!(method = %req.method(), outcome = "forbidden_origin", "authentication rejected");
            (
                StatusCode::FORBIDDEN,
                Json(json!({"error": "forbidden_origin"})),
            )
                .into_response()
        }
        Decision::Unauthorized => {
            debug!(method = %req.method(), outcome = "unauthorized", "authentication rejected");
            unauthorized_response()
        }
    }
}

/// 中间件决策内核（纯函数化供中间件与测试共用；WS 升级因路由同在受保护
/// 分组内同样经此判定——升级握手完成前 401/403 拒绝，无需真实建连即可验证）。
/// WS 判据 = `Upgrade: websocket` 头（浏览器禁改该头，REST 请求天然不携带；
/// 跨站页面发起 WS 时 Origin 必然是外站 → 与 Host 不一致被 403）。
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Admit,
    Forbidden,
    Unauthorized,
}

fn admit(
    auth: &AuthState,
    method: &Method,
    headers: &HeaderMap,
    remote: Option<SocketAddr>,
) -> Decision {
    // 1. 回环管理通道：token 匹配的本地请求无 Cookie，先于一切拦截放行
    if auth.admin_token_admits(remote, header_str(headers, ADMIN_TOKEN_HEADER)) {
        return Decision::Admit;
    }
    // 2. 同源防护（WS 升级即使是 GET 也必须校验——建连同样是状态敏感动作）
    if (is_state_changing(method) || is_ws_upgrade(headers)) && !origin_allows_headers(headers) {
        return Decision::Forbidden;
    }
    // 3. Cookie 会话
    match AuthState::extract_sid(headers).and_then(|sid| auth.validate(&sid)) {
        Some(_) => Decision::Admit,
        None => Decision::Unauthorized,
    }
}

fn header_str(headers: &HeaderMap, name: impl header::AsHeaderName) -> Option<&str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn is_ws_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// 重复 Origin/Host 头按畸形请求拒绝，避免不同 HTTP 栈对重复头取首值/末值
/// 不一致而产生同源校验绕过。
pub fn origin_allows_headers(headers: &HeaderMap) -> bool {
    if headers.get_all(header::ORIGIN).iter().count() > 1
        || headers.get_all(header::HOST).iter().count() > 1
    {
        return false;
    }
    origin_allows(
        header_str(headers, header::ORIGIN),
        header_str(headers, header::HOST),
    )
}

pub(crate) fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
        .into_response()
}

/// 小时级过期清扫任务（build_router 内 spawn）
pub(crate) fn spawn_sweeper(auth: std::sync::Arc<AuthState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SWEEP_PERIOD).await;
            auth.sweep();
        }
    });
}

// ---------- 来源 IP 注入 ----------

/// 管理口令来源链路：环境变量 GAMER_ADMIN_PASSWORD（最高）>
/// 环境变量 GAMER_ADMIN_PASSWORD_FILE 指向的密钥文件 > config [auth].password_hash >
/// 开发模式内置默认值。
pub fn resolve_credential(cfg: &crate::config::Config) -> Credential {
    resolve_credential_for_profile(cfg, crate::config::Profile::from_env())
}

/// 按显式 profile 解析管理凭据。生产环境不接受配置文件中的明文或旧
/// SHA-256，也不允许非法 password_hash 静默回落到明文；异常配置统一
/// 进入 [`Credential::Unavailable`]，使认证 fail closed。
pub fn resolve_credential_for_profile(
    cfg: &crate::config::Config,
    profile: crate::config::Profile,
) -> Credential {
    resolve_credential_with_password(
        cfg,
        profile,
        std::env::var("GAMER_ADMIN_PASSWORD").ok().as_deref(),
        std::env::var("GAMER_ADMIN_PASSWORD_FILE").ok().as_deref(),
    )
}

fn resolve_credential_with_password(
    cfg: &crate::config::Config,
    profile: crate::config::Profile,
    env_password: Option<&str>,
    env_password_file: Option<&str>,
) -> Credential {
    if let Some(p) = env_password.map(str::trim).filter(|p| !p.is_empty()) {
        return Credential::EnvPassword(p.to_string());
    }
    if let Some(path) = env_password_file.map(str::trim).filter(|p| !p.is_empty()) {
        match read_secret_file(path) {
            Ok(secret) => return Credential::SecretFile(secret),
            Err(e) => {
                warn!("管理密码密钥文件读取失败，认证已 fail closed：{}", e);
                return Credential::Unavailable;
            }
        }
    }
    if !cfg.auth.password_hash.trim().is_empty() {
        match parse_password_hash(cfg.auth.password_hash.trim()) {
            Ok(Credential::Argon2(encoded)) => return Credential::Argon2(encoded),
            Ok(Credential::Hash { .. }) if profile == crate::config::Profile::Prod => {
                warn!("生产模式拒绝旧 SHA-256 管理凭据；请改用 Argon2id password_hash");
                return Credential::Unavailable;
            }
            Ok(c) => return c,
            Err(_) => {
                warn!("管理 password_hash 配置非法，认证已 fail closed");
                return Credential::Unavailable;
            }
        }
    }
    if profile == crate::config::Profile::Prod {
        warn!(
            "生产模式未配置 GAMER_ADMIN_PASSWORD、GAMER_ADMIN_PASSWORD_FILE 或 Argon2id password_hash，认证已 fail closed"
        );
        return Credential::Unavailable;
    }
    Credential::Plain("admin123".to_string())
}

fn read_secret_file(path: &str) -> Result<String, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
    let secret = raw.trim().to_string();
    if secret.is_empty() {
        return Err(format!("{}: 密钥文件内容为空", path));
    }
    Ok(secret)
}

/// 回环管理通道令牌：GAMER_ADMIN_TOKEN 优先；dev 缺省自动生成并以 WARN 提示
/// 通道已启用，但**绝不把令牌值写入日志**（日志可能被导出或集中收集）；
/// prod 未设置则通道直接禁用。
pub fn resolve_admin_token(profile: crate::config::Profile) -> Option<String> {
    if let Ok(t) = std::env::var("GAMER_ADMIN_TOKEN") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    match profile {
        crate::config::Profile::Dev => {
            let t = random_hex_id(16); // 128bit
            warn!("{DEV_ADMIN_TOKEN_NOTICE}");
            Some(t)
        }
        crate::config::Profile::Prod => {
            warn!("GAMER_ADMIN_TOKEN 未设置：回环管理通道已禁用（gamer.ps1 优雅停机将退化为兜底强杀）");
            None
        }
    }
}

/// 来源 IP 文本（登录限流键）。最外层中间件从 ConnectInfo 提取后写入扩展，
/// 处理器经 Extension 取用；拿不到 ConnectInfo 时记 "unknown"（同一兜底桶）。
#[derive(Clone, Debug)]
pub struct IpKey(pub String);

pub(crate) async fn inject_ip_key(mut req: Request<Body>, next: Next) -> Response {
    let key = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    req.extensions_mut().insert(IpKey(key));
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(
        credential: Credential,
        idle_secs: u64,
        abs_secs: u64,
        max_fails: u32,
        window_secs: u64,
    ) -> AuthState {
        AuthState::new(
            credential,
            AuthConfig {
                session_abs_secs: abs_secs,
                session_idle_secs: idle_secs,
                login_max_fails: max_fails,
                login_window_secs: window_secs,
                password_hash: String::new(),
            },
            false,
            None,
        )
    }

    fn sleep_ms(ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    #[test]
    fn credentials_chain_plain_env_hash() {
        // 明文链路
        let st = state(Credential::Plain("admin123".into()), 60, 60, 10, 300);
        assert!(st.verify_credentials("admin123"));
        assert!(!st.verify_credentials("wrong"));

        // 环境变量链路与 hash 链路的 digest 内容一致性
        let st2 = state(Credential::EnvPassword("s3cret!".into()), 60, 60, 10, 300);
        assert!(st2.verify_credentials("s3cret!"));
        assert!(!st2.verify_credentials("admin123"));

        let st3 = state(
            Credential::SecretFile("file-secret".into()),
            60,
            60,
            10,
            300,
        );
        assert!(st3.verify_credentials("file-secret"));
        assert!(!st3.verify_credentials("admin123"));
    }

    #[test]
    fn hash_format_roundtrip_and_reject() {
        // sha256(salt||pw) 的 hex 装回去要能验过
        let salt_hex = "00112233445566778899aabb";
        let salt = decode_hex(salt_hex).unwrap();
        let digest = {
            let mut m = Sha256::new();
            m.update(&salt);
            m.update(b"hunter2");
            let out: [u8; 32] = m.finalize().into();
            out.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let boxed = format!("sha256${salt_hex}${digest}");
        let parsed = parse_password_hash(&boxed).unwrap();
        let st = state(parsed, 60, 60, 10, 300);
        assert!(st.verify_credentials("hunter2"));
        assert!(!st.verify_credentials("hunter"));

        for bad in ["plain", "sha256$aa$bb", "sha256$aa$", "", "md5$aabbccdd$ab"] {
            assert!(parse_password_hash(bad).is_err(), "{bad} 不应解析成功");
        }
    }

    #[test]
    fn argon2id_hash_roundtrip_and_wrong_password() {
        let encoded = hash_password("argon-secret").unwrap();
        assert!(encoded.starts_with("$argon2id$"), "{encoded}");
        let credential = parse_password_hash(&encoded).unwrap();
        assert!(matches!(credential, Credential::Argon2(_)));
        let st = state(credential, 60, 60, 10, 300);
        assert!(st.verify_credentials("argon-secret"));
        assert!(!st.verify_credentials("wrong-password"));
    }

    #[test]
    fn argon2id_hash_format_and_parameter_boundaries_are_rejected() {
        for bad in [
            "$argon2i$v=19$m=19456,t=2,p=1$c2FsdA$YWJjZA",
            "$argon2id$v=19$m=19456,t=2,p=1$not-base64$not-base64",
            "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$",
            "$argon2id$v=18$m=19456,t=2,p=1$c2FsdA$YWJjZA",
            "$argon2id$v=19$m=0,t=2,p=1$c2FsdA$YWJjZA",
        ] {
            assert!(parse_password_hash(bad).is_err(), "{bad} 不应解析成功");
        }
    }

    #[test]
    fn legacy_credentials_upgrade_in_memory_after_successful_login() {
        let salt = vec![0x42; 16];
        let mut digest_input = salt.clone();
        digest_input.extend_from_slice(b"legacy-secret");
        let digest = sha256(&digest_input).to_vec();
        let st = state(Credential::Hash { salt, digest }, 60, 60, 10, 300);

        assert!(st.verify_credentials("legacy-secret"));
        let (sid, _) = st
            .attempt_login("admin", "legacy-secret", "legacy-ip")
            .unwrap();
        assert!(st.validate(&sid).is_some());
        assert!(matches!(
            &*st.credential.read().unwrap(),
            Credential::Argon2(encoded) if encoded.starts_with("$argon2id$")
                && !encoded.contains("legacy-secret")
        ));
        assert!(st.verify_credentials("legacy-secret"));
        assert!(!st.verify_credentials("wrong-password"));
    }

    #[test]
    fn legacy_plain_password_is_not_retained_after_successful_login() {
        let st = state(
            Credential::Plain("legacy-plain-secret".into()),
            60,
            60,
            10,
            300,
        );
        let (sid, _) = st
            .attempt_login("admin", "legacy-plain-secret", "plain-ip")
            .unwrap();
        assert!(st.validate(&sid).is_some());
        assert!(matches!(
            &*st.credential.read().unwrap(),
            Credential::Argon2(encoded) if !encoded.contains("legacy-plain-secret")
        ));
    }

    #[test]
    fn session_lifecycle_absolute_and_sliding() {
        let st = state(
            Credential::Plain("pw".into()),
            /*idle*/ 1_000_000,
            /*abs*/ 1,
            10,
            300,
        );
        let (sid, user) = st.attempt_login("admin", "pw", "ipA").unwrap();
        assert_eq!(user, "admin");
        assert_eq!(st.validate(&sid).as_deref(), Some("admin"));
        // 绝对 TTL 到期：sleep 让 abs(1s) 过期
        sleep_ms(1100);
        assert_eq!(st.validate(&sid), None, "绝对有效期到期必须强制重登");
        assert_eq!(st.sessions_len(), 0);
    }

    #[test]
    fn session_sliding_idle_expires_and_renews() {
        let st = state(
            Credential::Plain("pw".into()),
            /*idle*/ 1,
            /*abs*/ 10_000,
            10,
            300,
        );
        let (sid, _) = st.attempt_login("admin", "pw", "ipB").unwrap();
        sleep_ms(1200);
        assert_eq!(st.validate(&sid), None, "空闲超期未活动应失效");

        // 活动即续期：idle=1s 下每 600ms 探一次，远小于 abs，永远活着
        let st2 = state(Credential::Plain("pw".into()), 2, 10_000, 10, 300);
        let (sid2, _) = st2.attempt_login("admin", "pw", "ipC").unwrap();
        for _ in 0..4 {
            sleep_ms(600);
            assert_eq!(
                st2.validate(&sid2).as_deref(),
                Some("admin"),
                "活跃期内不应被空闲判定清除"
            );
        }
    }

    #[test]
    fn logout_destroys_immediately() {
        let st = state(Credential::Plain("pw".into()), 100, 100, 10, 300);
        let (sid, _) = st.attempt_login("admin", "pw", "ipD").unwrap();
        st.destroy(&sid);
        assert_eq!(st.validate(&sid), None);
        st.destroy(&sid); // 幂等
    }

    #[test]
    fn rate_limit_kicks_in_after_max_fails_and_resets_on_success() {
        // 锁定窗口内：连续失败达到上限后全部 429（正确口令也拒），他 IP 不受牵连
        let st = state(
            Credential::Plain("right".into()),
            100,
            100,
            /*max*/ 3,
            /*window*/ 3600,
        );
        for _ in 0..3 {
            assert!(matches!(
                st.attempt_login("admin", "bad", "9.9.9.9"),
                Err(LoginError::Invalid)
            ));
        }
        match st.attempt_login("admin", "bad", "9.9.9.9") {
            Err(LoginError::RateLimited { retry_after_secs }) => assert!(retry_after_secs >= 1),
            other => panic!("应触发限流，got {other:?}"),
        }
        assert!(matches!(
            st.attempt_login("admin", "right", "9.9.9.9"),
            Err(LoginError::RateLimited { .. })
        ));
        assert!(st.attempt_login("admin", "right", "5.5.5.5").is_ok());

        // 滑动窗口滑出后解锁（1s 窗口可实测）
        let small = state(Credential::Plain("right".into()), 100, 100, 2, 1);
        small.attempt_login("admin", "bad", "7.7.7.7").unwrap_err();
        small.attempt_login("admin", "bad", "7.7.7.7").unwrap_err();
        assert!(matches!(
            small.attempt_login("admin", "right", "7.7.7.7"),
            Err(LoginError::RateLimited { .. }) // 锁定期正确口令同样拒
        ));
        sleep_ms(1200);
        assert!(matches!(
            small.attempt_login("admin", "bad", "7.7.7.7"),
            Err(LoginError::Invalid) // 已解锁但口令仍错
        ));

        // 成功登录清空该来源失败记录：解锁后失败一次，成功登录重置计数，
        // 再失败一次仍是 invalid（若未清空则已是 max=1 的第二个 → 429）
        let tight = state(
            Credential::Plain("right".into()),
            100,
            100,
            /*max*/ 2,
            3600,
        );
        assert!(matches!(
            tight.attempt_login("admin", "bad", "8.8.8.8"),
            Err(LoginError::Invalid)
        ));
        assert!(tight.attempt_login("admin", "right", "8.8.8.8").is_ok());
        assert!(matches!(
            tight.attempt_login("admin", "bad", "8.8.8.8"),
            Err(LoginError::Invalid) // 计数已被成功登录清空，未触发限流
        ));
    }

    #[test]
    fn rate_limit_bucket_isolated_by_ip_and_username_pair() {
        let st = state(Credential::Plain("right".into()), 100, 100, 2, 3600);

        // 同一 IP 的无关用户名达到上限，不得误锁唯一合法管理员用户名。
        for _ in 0..2 {
            assert!(matches!(
                st.attempt_login("decoy", "bad", "203.0.113.10"),
                Err(LoginError::Invalid)
            ));
        }
        assert!(matches!(
            st.attempt_login("decoy", "right", "203.0.113.10"),
            Err(LoginError::RateLimited { .. })
        ));
        assert!(
            st.attempt_login("admin", "right", "203.0.113.10").is_ok(),
            "相同 IP、不同用户名必须使用独立失败桶"
        );

        // 同一用户名在一个 IP 被锁后，另一 IP 仍可正常认证。
        for _ in 0..2 {
            assert!(matches!(
                st.attempt_login("admin", "bad", "203.0.113.20"),
                Err(LoginError::Invalid)
            ));
        }
        assert!(matches!(
            st.attempt_login("admin", "right", "203.0.113.20"),
            Err(LoginError::RateLimited { .. })
        ));
        assert!(
            st.attempt_login("admin", "right", "203.0.113.21").is_ok(),
            "相同用户名、不同 IP 必须使用独立失败桶"
        );
    }

    #[test]
    fn admin_token_loopback_only() {
        let mut st = state(Credential::Plain("pw".into()), 100, 100, 10, 300);
        // 注入 token
        st.admin_token = Some("tok123".into());
        let loopback: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let lan: SocketAddr = "192.168.1.9:40000".parse().unwrap();
        assert!(st.admin_token_admits(Some(loopback), Some("tok123")));
        assert!(
            !st.admin_token_admits(Some(lan), Some("tok123")),
            "非回环同 token 必须拒绝"
        );
        assert!(
            !st.admin_token_admits(None, Some("tok123")),
            "缺 ConnectInfo 拒绝"
        );
        assert!(!st.admin_token_admits(Some(loopback), None), "缺头拒绝");
        assert!(!st.admin_token_admits(Some(loopback), Some("wrong")));
        st.admin_token = None;
        assert!(
            !st.admin_token_admits(Some(loopback), Some("tok123")),
            "通道关闭一律拒绝"
        );
    }

    #[test]
    fn origin_host_matrix() {
        assert!(origin_allows(None, Some("localhost:8443")), "缺失放行 CLI");
        assert!(origin_allows(
            Some("http://localhost:5173"),
            Some("localhost:5173")
        ));
        assert!(
            origin_allows(Some("http://LocalHost:5173"), Some("localhost:5173")),
            "大小写不敏感"
        );
        assert!(!origin_allows(
            Some("http://evil.example"),
            Some("localhost:8443")
        ));
        assert!(!origin_allows(Some("null"), Some("localhost:8443")));
        assert!(
            !origin_allows(Some("http://evil.example"), None),
            "有 Origin 无 Host 拒绝"
        );
        assert!(!origin_allows(
            Some("https://localhost:8443.evil.com"),
            Some("localhost:8443")
        ));
        // 前缀撞库防误伤：authority 必须整体一致
        assert!(!origin_allows(
            Some("http://localhost:8443.evil"),
            Some("localhost:8443")
        ));
        // Origin 必须是浏览器的 http(s) origin；畸形 scheme、空 authority、
        // 用户信息和多值头均不能借字符串前缀比较绕过同源校验。
        assert!(!origin_allows(
            Some("ftp://localhost:8443"),
            Some("localhost:8443")
        ));
        assert!(!origin_allows(
            Some("http:///localhost:8443"),
            Some("localhost:8443")
        ));
        assert!(!origin_allows(
            Some("http://user@localhost:8443"),
            Some("localhost:8443")
        ));
        assert!(!origin_allows(
            Some("http://localhost:8443, http://evil.example"),
            Some("localhost:8443")
        ));
        assert!(!origin_allows(Some("http://localhost:8443"), None));
    }

    #[test]
    fn generated_admin_token_is_not_exposed_by_log_message_contract() {
        // 令牌仍需返回给进程内的管理通道，但其值只能存在于内存，不能拼入
        // 日志消息。这个断言与生产日志格式保持同一白名单：只允许头名称和提示，
        // 禁止把随机值回显到日志文本。
        let token = random_hex_id(16);
        assert_eq!(token.len(), 32);
        let safe_message = DEV_ADMIN_TOKEN_NOTICE;
        assert!(!safe_message.contains(&token));
        for sensitive in ["Cookie", "Authorization", "password", "token", "zip"] {
            assert!(
                !safe_message
                    .to_ascii_lowercase()
                    .contains(&sensitive.to_ascii_lowercase()),
                "日志提示不得出现敏感字段名 {sensitive:?}"
            );
        }
    }

    #[test]
    fn origin_headers_reject_duplicate_security_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "http://localhost:8443".parse().unwrap());
        headers.insert(header::HOST, "localhost:8443".parse().unwrap());
        assert!(origin_allows_headers(&headers));

        headers.append(header::ORIGIN, "http://evil.example".parse().unwrap());
        assert!(!origin_allows_headers(&headers));

        let mut host_dupe = HeaderMap::new();
        host_dupe.insert(header::ORIGIN, "http://localhost:8443".parse().unwrap());
        host_dupe.insert(header::HOST, "localhost:8443".parse().unwrap());
        host_dupe.append(header::HOST, "evil.example".parse().unwrap());
        assert!(!origin_allows_headers(&host_dupe));
    }

    #[test]
    fn production_credential_resolution_fails_closed_without_strong_secret() {
        let mut cfg = crate::config::Config::default();
        cfg.auth.password_hash = "sha256$aabbccdd11223344$".to_string() + &"ab".repeat(32);

        let legacy =
            resolve_credential_with_password(&cfg, crate::config::Profile::Prod, None, None);
        assert!(matches!(legacy, Credential::Unavailable));
        assert!(
            !AuthState::new(legacy, Default::default(), true, None).verify_credentials("admin123")
        );

        cfg.auth.password_hash = "not-a-password-hash".into();
        assert!(matches!(
            resolve_credential_with_password(&cfg, crate::config::Profile::Prod, None, None),
            Credential::Unavailable
        ));

        cfg.auth.password_hash.clear();
        assert!(matches!(
            resolve_credential_with_password(&cfg, crate::config::Profile::Prod, None, None),
            Credential::Unavailable
        ));
    }

    #[test]
    fn environment_password_has_priority_over_secret_file_and_config_hash() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-auth-secret-{}-{}",
            std::process::id(),
            random_hex_id(8)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let secret_path = dir.join("admin.secret");
        std::fs::write(&secret_path, "file-secret\n").unwrap();

        let mut cfg = crate::config::Config::default();
        cfg.auth.password_hash = hash_password("config-secret").unwrap();
        let credential = resolve_credential_with_password(
            &cfg,
            crate::config::Profile::Prod,
            Some("env-secret"),
            Some(secret_path.to_str().unwrap()),
        );
        assert!(matches!(credential, Credential::EnvPassword(ref value) if value == "env-secret"));
        let st = AuthState::new(credential, Default::default(), true, None);
        assert!(st.verify_credentials("env-secret"));
        assert!(!st.verify_credentials("config-secret"));

        let credential2 = resolve_credential_with_password(
            &cfg,
            crate::config::Profile::Prod,
            None,
            Some(secret_path.to_str().unwrap()),
        );
        assert!(matches!(credential2, Credential::SecretFile(ref value) if value == "file-secret"));
        let st2 = AuthState::new(credential2, Default::default(), true, None);
        assert!(st2.verify_credentials("file-secret"));
        assert!(!st2.verify_credentials("config-secret"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_file_resolution_fails_fast_on_missing_or_empty_content() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-auth-secret-fail-{}-{}",
            std::process::id(),
            random_hex_id(8)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.secret");
        let empty = dir.join("empty.secret");
        std::fs::write(&empty, "   \n").unwrap();

        let cfg = crate::config::Config::default();
        assert!(matches!(
            resolve_credential_with_password(
                &cfg,
                crate::config::Profile::Prod,
                None,
                Some(missing.to_str().unwrap()),
            ),
            Credential::Unavailable
        ));
        assert!(matches!(
            resolve_credential_with_password(
                &cfg,
                crate::config::Profile::Prod,
                None,
                Some(empty.to_str().unwrap()),
            ),
            Credential::Unavailable
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn dev_default_does_not_leak_secret_values_to_logs_or_responses() {
        let st = state(Credential::Plain("dev-secret".into()), 60, 60, 10, 300);
        assert_eq!(st.credential_source(), "dev:built-in-default");

        let resp = unauthorized_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("unauthorized response body should be readable");
        let body_text = std::str::from_utf8(&body).unwrap();
        assert_eq!(body_text, r#"{"error":"unauthorized"}"#);
        assert!(!body_text.contains("dev-secret"));
        assert!(!st.credential_source().contains("dev-secret"));
    }

    /// 测试专用决策入口：按参数拼装请求头后走与中间件相同的 admit 内核
    struct AdmitParts<'a> {
        origin: Option<&'a str>,
        host: Option<&'a str>,
        cookie: Option<&'a str>,
        ws_upgrade: bool,
        admin_token: Option<&'a str>,
        remote: Option<SocketAddr>,
    }

    fn admit_parts(st: &AuthState, method: Method, parts: AdmitParts<'_>) -> Decision {
        let mut hm = HeaderMap::new();
        if let Some(o) = parts.origin {
            hm.insert(
                header::ORIGIN,
                axum::http::HeaderValue::from_str(o).unwrap(),
            );
        }
        if let Some(h) = parts.host {
            hm.insert(header::HOST, axum::http::HeaderValue::from_str(h).unwrap());
        }
        if let Some(c) = parts.cookie {
            hm.insert(
                header::COOKIE,
                axum::http::HeaderValue::from_str(c).unwrap(),
            );
        }
        if parts.ws_upgrade {
            hm.insert(
                header::UPGRADE,
                axum::http::HeaderValue::from_static("websocket"),
            );
        }
        if let Some(t) = parts.admin_token {
            let name = axum::http::HeaderName::from_static("x-admin-token");
            hm.insert(name, axum::http::HeaderValue::from_str(t).unwrap());
        }
        admit(st, &method, &hm, parts.remote)
    }

    #[test]
    fn decision_matrix_ws_origin_token_cookie() {
        let mut st = state(Credential::Plain("pw".into()), 100, 100, 10, 300);
        st.admin_token = Some("tok".into());
        let (sid, _) = st.attempt_login("admin", "pw", "ipZ").unwrap();
        let cookie = format!("{SESSION_COOKIE}={sid}");
        let lb: SocketAddr = "127.0.0.1:1111".parse().unwrap();

        // 普通 API：无 cookie → 401
        assert_eq!(
            admit_parts(
                &st,
                Method::GET,
                AdmitParts {
                    origin: None,
                    host: Some("h"),
                    cookie: None,
                    ws_upgrade: false,
                    admin_token: None,
                    remote: None,
                },
            ),
            Decision::Unauthorized
        );
        // 有效 cookie 放行
        assert_eq!(
            admit_parts(
                &st,
                Method::GET,
                AdmitParts {
                    origin: None,
                    host: Some("h"),
                    cookie: Some(&cookie),
                    ws_upgrade: false,
                    admin_token: None,
                    remote: None,
                },
            ),
            Decision::Admit
        );
        // POST 有 cookie 但 Origin 违规 → 403
        assert_eq!(
            admit_parts(
                &st,
                Method::POST,
                AdmitParts {
                    origin: Some("http://evil"),
                    host: Some("h"),
                    cookie: Some(&cookie),
                    ws_upgrade: false,
                    admin_token: None,
                    remote: None,
                },
            ),
            Decision::Forbidden
        );
        // GET 带 evil Origin：非状态变更方法，契约外放行
        assert_eq!(
            admit_parts(
                &st,
                Method::GET,
                AdmitParts {
                    origin: Some("http://evil"),
                    host: Some("h"),
                    cookie: Some(&cookie),
                    ws_upgrade: false,
                    admin_token: None,
                    remote: None,
                },
            ),
            Decision::Admit
        );
        // WS 升级即使 cookie 合法也强制 Origin 一致
        assert_eq!(
            admit_parts(
                &st,
                Method::GET,
                AdmitParts {
                    origin: Some("http://evil"),
                    host: Some("h"),
                    cookie: Some(&cookie),
                    ws_upgrade: true,
                    admin_token: None,
                    remote: None,
                },
            ),
            Decision::Forbidden
        );
        // WS 缺 Origin（CLI 场景）但 guard 层仍要求会话
        assert_eq!(
            admit_parts(
                &st,
                Method::GET,
                AdmitParts {
                    origin: None,
                    host: Some("h"),
                    cookie: None,
                    ws_upgrade: true,
                    admin_token: None,
                    remote: None,
                },
            ),
            Decision::Unauthorized
        );
        // 回环 + 正确 token：无 cookie/origin 直达放行
        assert_eq!(
            admit_parts(
                &st,
                Method::POST,
                AdmitParts {
                    origin: None,
                    host: None,
                    cookie: None,
                    ws_upgrade: false,
                    admin_token: Some("tok"),
                    remote: Some(lb),
                },
            ),
            Decision::Admit
        );
        // 非回环同 token 拒绝且回落会话判定 → 401
        let lan: SocketAddr = "10.0.0.2:5".parse().unwrap();
        assert_eq!(
            admit_parts(
                &st,
                Method::POST,
                AdmitParts {
                    origin: None,
                    host: None,
                    cookie: None,
                    ws_upgrade: false,
                    admin_token: Some("tok"),
                    remote: Some(lan),
                },
            ),
            Decision::Unauthorized
        );
    }

    #[test]
    fn cookie_extraction_edge_cases() {
        let mut hm = HeaderMap::new();
        assert_eq!(AuthState::extract_sid(&hm), None);
        hm.insert(
            header::COOKIE,
            "other=x; gb_session=abc123".parse().unwrap(),
        );
        assert_eq!(AuthState::extract_sid(&hm).as_deref(), Some("abc123"));
        hm.insert(header::COOKIE, "gb_session=".parse().unwrap());
        assert_eq!(AuthState::extract_sid(&hm), None, "空值视作缺失");
        hm.insert(
            header::COOKIE,
            "gb_session_prefixed=1; gb_session=z".parse().unwrap(),
        );
        assert_eq!(AuthState::extract_sid(&hm).as_deref(), Some("z"));
    }

    #[test]
    fn ct_eq_semantics() {
        assert!(ct_eq(b"", b""));
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abd", b"abc"));
        assert!(!ct_eq(b"abcd", b"abc"));
        assert!(!ct_eq(b"abc", b"abcdef"));
    }
}
