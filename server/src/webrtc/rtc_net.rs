//! WebRTC 媒体链路的 ICE 候选外部宣告（容器 / NAT 1-to-1 部署适配）。
//!
//! 背景：Docker bridge 部署时服务端 host candidate 是容器内网 IP（172.x），
//! 宿主浏览器无法路由 → 信令（WS）正常但媒体（RTP/UDP）协商不出候选对 →
//! 投屏黑屏。经典 NAT 1-to-1 场景用 config.toml 三个键宣告「宿主可达的
//! 外部地址」：
//!
//! - `rtc_external_ip`：候选一律宣告该 IP（`set_nat_1to1_ips(.., Host)` 语义，
//!   不经 STUN/接口枚举，容器内网 IP 不再宣告）；
//! - `rtc_udp_port`：媒体 UDP 换单 socket UDPMux 固定绑该端口（容器端口映射
//!   的前提；0 = 既有行为：每会话临时端口）。**必须与 rtc_external_ip 成对
//!   配置**（config 启动校验强制）；
//! - `rtc_external_port`：候选宣告该端口（docker -p 的宿主侧端口）。0 = 宣告
//!   rtc_udp_port 本身（容器内外同端口号映射 -p A:A/udp）。
//!
//! 容器配法示例（docker -p B:A/udp，宿主 B → 容器 A）：
//!
//! ```toml
//! rtc_external_ip = "192.168.1.10"   # 浏览器可达的宿主 IP
//! rtc_udp_port = 3478                # 容器内绑定端口 A
//! rtc_external_port = 50000          # 宿主对外端口 B（B == A 时配 0）
//! # docker run ... -p 50000:3478/udp
//! ```
//!
//! 实现注记（webrtc-rs 0.13 / webrtc-ice 0.13）：`set_nat_1to1_ips` 只重写
//! 候选 **IP**，候选**地址/端口**的另一来源是 mux conn 的 `local_addr()`
//! （UDPMuxDefault::create_muxed_conn 缓存，muxed gather 直接消费）。「绑 A
//! 宣 B」因此经自定义 [`AdvertisedAddrConn`] 实现：tokio UdpSocket 包一层、
//! `local_addr()` 汇报 `external_ip:advertised_port`；收发包全部委托真实
//! socket，STUN 应答回源地址，docker DNAT 对链路透明。nat1to1 对已是
//! external_ip 的候选地址替换幂等（external_ip → external_ip）。
//!
//! **回归注记（2026-08-29 容器实测）**：早期实现汇报 `0.0.0.0:<port>`——
//! unspecified IP 使 muxed gather 无法生成有效本地候选（候选收集 0，ICE
//! 停在 no candidate pairs）。`local_addr()` 必须返回**具体 IP**，回归测试
//! `gather_via_agent_mux_*` 用真实 agent + mux 跑 gather 产物兜底。
//!
//! 三键全缺省 → [`build_rtc_setting_engine`] 仅带 mDNS Disabled（其余零
//! 配置），Windows 直跑 / 既有部署除候选不再宣告 .local 外行为不变。

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use async_trait::async_trait;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::ice::udp_mux::{UDPMuxDefault, UDPMuxParams};
use webrtc::ice::udp_network::UDPNetwork;
use webrtc::ice_transport::ice_candidate_type::RTCIceCandidateType;
use webrtc::util::conn::Conn;

use crate::config::Config;

/// 候选宣告端口：显式 rtc_external_port 优先，否则宣告绑定端口本身
/// （要求容器内外同端口号映射，如 -p 3478:3478/udp）。
/// rtc_udp_port == 0（未用固定端口）时返回 0，调用方不会走到 socket 绑定。
pub(crate) fn advertised_port(bind: u16, external: u16) -> u16 {
    if external != 0 {
        external
    } else {
        bind
    }
}

/// UDPMux 底层 conn 包装：UDP 收发全部委托真实 socket，仅 `local_addr()`
/// 汇报「对外宣告地址」（具体 external_ip:advertised_port，见模块回归注记）。
/// muxed gather 从该值取候选地址与端口——IP 必须具体（0.0.0.0 会得到
/// 零候选），端口由此实现「容器内绑 A、候选宣 B」的映射语义。
struct AdvertisedAddrConn {
    sock: tokio::net::UdpSocket,
    advertised: SocketAddr,
}

impl AdvertisedAddrConn {
    fn new(sock: tokio::net::UdpSocket, advertised: SocketAddr) -> Self {
        Self { sock, advertised }
    }
}

#[async_trait]
impl Conn for AdvertisedAddrConn {
    async fn connect(&self, addr: SocketAddr) -> webrtc::util::Result<()> {
        Ok(self.sock.connect(addr).await?)
    }

    async fn recv(&self, buf: &mut [u8]) -> webrtc::util::Result<usize> {
        Ok(self.sock.recv(buf).await?)
    }

    async fn recv_from(&self, buf: &mut [u8]) -> webrtc::util::Result<(usize, SocketAddr)> {
        Ok(self.sock.recv_from(buf).await?)
    }

    async fn send(&self, buf: &[u8]) -> webrtc::util::Result<usize> {
        Ok(self.sock.send(buf).await?)
    }

    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> webrtc::util::Result<usize> {
        Ok(self.sock.send_to(buf, target).await?)
    }

    fn local_addr(&self) -> webrtc::util::Result<SocketAddr> {
        Ok(self.advertised)
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }

    async fn close(&self) -> webrtc::util::Result<()> {
        Ok(())
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

/// 进程级共享媒体 UDP mux：UDPMux 按 ICE ufrag 复用单 socket，多设备/多
/// viewer 共用一个固定端口；固定端口绑定只能发生一次，故懒初始化后全程
/// 持有（端口相关配置变更需重启生效，与 compute_max_concurrency 同口径）。
static SHARED_UDP_MUX: tokio::sync::OnceCell<Arc<UDPMuxDefault>> =
    tokio::sync::OnceCell::const_new();

/// 对外宣告 IP：rtc_external_ip（config 校验保证 rtc_udp_port 非 0 时必配且
/// 为合法 IP 字面量）。parse 失败兜底回落真实绑定地址（load_from 校验后的
/// 配置不会走到），避免连接建立被配置解析二次阻断。
fn advertise_ip(cfg: &Config, sock: &tokio::net::UdpSocket) -> IpAddr {
    let ext = cfg.rtc_external_ip.trim();
    match ext.parse::<IpAddr>() {
        Ok(ip) => ip,
        Err(_) => sock
            .local_addr()
            .map(|a| a.ip())
            .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED.into()),
    }
}

async fn shared_udp_mux(cfg: &Config) -> anyhow::Result<Arc<UDPMuxDefault>> {
    SHARED_UDP_MUX
        .get_or_try_init(|| async {
            let sock = tokio::net::UdpSocket::bind(("0.0.0.0", cfg.rtc_udp_port))
                .await
                .map_err(|e| {
                    anyhow::anyhow!("rtc_udp_port={} UDP 绑定失败：{e}", cfg.rtc_udp_port)
                })?;
            let advertised = SocketAddr::new(
                advertise_ip(cfg, &sock),
                advertised_port(cfg.rtc_udp_port, cfg.rtc_external_port),
            );
            let conn = AdvertisedAddrConn::new(sock, advertised);
            Ok::<_, anyhow::Error>(UDPMuxDefault::new(UDPMuxParams::new(conn)))
        })
        .await
        .cloned()
}

/// 按 rtc_* 配置构造 SettingEngine（恒返回 Some；viewer 构建路径统一带
/// SettingEngine）。
///
/// - **mDNS 一律 Disabled**（关键，2026-08-30 实证）：webrtc-rs 默认
///   QueryAndGather 会把 answer 的 host 候选宣告成 `xxx.local`，浏览器必须
///   经 mDNS 解析才能发起 ICE 检查——Windows 同机部署下这条解析链间歇性
///   失效（防火墙/组播/网卡变化），表现即建连风暴（每 ~4.2s 一轮、连败十
///   几次后自愈）。Disabled 后 answer 带明文 IP，浏览器检查直达（prflx 回
///   路，实测秒连）；对端 offer 的 mDNS 候选无需解析——本 crate 版本
///   webrtc-rs 从不解析远端 .local 候选（resolved_addr 恒 0.0.0.0:0），连接
///   本就只靠浏览器侧检查驱动。日志里 `discard success message ... no such
///   remote` 即该死路径的噪音，可忽略。
/// - `rtc_external_ip` 非空：`set_nat_1to1_ips(.., Host)`——host 候选一律
///   宣告该 IP。mDNS 已 Disabled，不存在 nat1to1×mDNS 冲突。
/// - `rtc_udp_port` 非 0：媒体 UDP 换单 socket UDPMux 固定绑端口（进程级
///   共享，见 [`shared_udp_mux`]）；候选地址/端口 = mux conn 的
///   `local_addr()`（即 external_ip + [`advertised_port`]，见模块注记）。
pub(crate) async fn build_rtc_setting_engine(cfg: &Config) -> anyhow::Result<SettingEngine> {
    let nat_ip = cfg.rtc_external_ip.trim();

    let mut se = SettingEngine::default();
    se.set_ice_multicast_dns_mode(webrtc::ice::mdns::MulticastDnsMode::Disabled);
    if !nat_ip.is_empty() {
        se.set_nat_1to1_ips(vec![nat_ip.to_string()], RTCIceCandidateType::Host);
    }
    if cfg.rtc_udp_port != 0 {
        let mux = shared_udp_mux(cfg).await?;
        se.set_udp_network(UDPNetwork::Muxed(mux));
    }
    Ok(se)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertised_port_prefers_external_then_bind() {
        assert_eq!(advertised_port(3478, 50000), 50000, "显式对外端口优先");
        assert_eq!(advertised_port(3478, 0), 3478, "缺省宣告绑定端口本身");
        assert_eq!(advertised_port(0, 0), 0, "未配置 = 0，不会走到 socket");
    }

    #[tokio::test]
    async fn advertised_addr_conn_delegates_io_and_reports_advertised_addr() {
        // 真实 socket 走临时端口（不依赖固定端口可用性），宣告地址任意指定
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let real_port = sock.local_addr().unwrap().port();
        let advertised: SocketAddr = "203.0.113.10:40001".parse().unwrap();
        let conn = AdvertisedAddrConn::new(sock, advertised);

        // local_addr 必须原样汇报宣告地址（具体 IP，绝不 0.0.0.0——见模块回归注记）
        assert_eq!(conn.local_addr().unwrap(), advertised);
        assert_eq!(conn.remote_addr(), None);

        // send_to / recv_from 委托真实 socket：包装器发包，对端能收到
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();
        conn.send_to(b"ping", peer_addr).await.unwrap();
        let mut buf = [0u8; 16];
        let (len, from) = peer.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], b"ping");
        assert_eq!(from.port(), real_port, "线上端口仍是真实绑定端口");

        // recv 委托（connect 后的单端读）：对端向真实绑定地址回包
        conn.connect(peer_addr).await.unwrap();
        peer.send_to(b"pong", SocketAddr::from(([127, 0, 0, 1], real_port)))
            .await
            .unwrap();
        let mut buf2 = [0u8; 16];
        let n = conn.recv(&mut buf2).await.unwrap();
        assert_eq!(&buf2[..n], b"pong");
    }

    /// 跑一次真实 gather：agent + UDPMuxDefault(AdvertisedAddrConn)，
    /// 等 on_candidate(None)（gathering done）后取本地候选。这是容器
    /// 「候选收集 0」回归的直接断言面——单测 local_addr 值不够，必须看
    /// gather 产物。
    async fn gather_with_mux(
        mux: Arc<UDPMuxDefault>,
        nat_1to1: Option<String>,
    ) -> Vec<Arc<dyn webrtc::ice::candidate::Candidate + Send + Sync>> {
        use webrtc::ice::agent::agent_config::AgentConfig;
        use webrtc::ice::agent::Agent;
        use webrtc::ice::network_type::NetworkType;

        let agent = Agent::new(AgentConfig {
            udp_network: UDPNetwork::Muxed(mux),
            network_types: vec![NetworkType::Udp4],
            nat_1to1_ips: nat_1to1.clone().map(|ip| vec![ip]).unwrap_or_default(),
            // nat1to1 未配置时 Unspecified 由 ice 默认为 host，无需显式
            ..Default::default()
        })
        .await
        .unwrap();

        let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);
        agent.on_candidate(Box::new(move |c| {
            let tx = done_tx.clone();
            Box::pin(async move {
                if c.is_none() {
                    let _ = tx.send(()).await;
                }
            })
        }));
        agent.gather_candidates().unwrap();

        // gathering done 有超时兜底：gather 内部错误只记日志不阻断 None 信号，
        // 但保险起见不让测试无限挂起
        tokio::time::timeout(std::time::Duration::from_secs(10), done_rx.recv())
            .await
            .expect("gather timeout")
            .expect("done channel closed");

        let cands = agent.get_local_candidates().await.unwrap();
        agent.close().await.unwrap();
        cands
    }

    #[tokio::test]
    async fn gather_via_agent_mux_produces_external_ip_candidates() {
        // 回归：muxed gather 产物必须是 external_ip:advertised_port。
        // nat1to1(Host) 与生产配置一致；地址端口均来自 mux conn 的
        // local_addr() 汇报 + nat1to1 替换。
        let ext_ip = "203.0.113.10";
        let port = 51820u16;
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mux = UDPMuxDefault::new(UDPMuxParams::new(AdvertisedAddrConn::new(
            sock,
            format!("{ext_ip}:{port}").parse().unwrap(),
        )));

        let cands = gather_with_mux(mux, Some(ext_ip.to_string())).await;
        assert!(!cands.is_empty(), "muxed gather 必须产出本地候选");
        for c in &cands {
            assert_eq!(c.address(), ext_ip, "候选地址须为宣告的 external_ip");
            assert_eq!(c.port(), port, "候选端口须为宣告的 advertised_port");
        }
    }

    #[tokio::test]
    async fn gather_multi_ufrag_reuses_mux_with_concrete_addresses() {
        // 多候选场景：多个 agent（各自 ufrag）复用同一 mux——muxed 连接按
        // ufrag 分路，候选互不干扰且地址具体（绝不 0.0.0.0）。无 nat1to1：
        // 地址回落接口 IP，端口仍为宣告端口。
        let port = 51821u16;
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mux = UDPMuxDefault::new(UDPMuxParams::new(AdvertisedAddrConn::new(
            sock,
            format!("192.0.2.50:{port}").parse().unwrap(),
        )));

        for i in 0..2 {
            let cands = gather_with_mux(mux.clone(), None).await;
            assert!(!cands.is_empty(), "agent #{i}: mux 复用必须产出本地候选");
            for c in &cands {
                let addr: std::net::IpAddr = c.address().parse().unwrap_or_else(|_| {
                    panic!("agent #{i}: 候选地址须为合法 IP，got {}", c.address())
                });
                assert!(
                    !addr.is_unspecified(),
                    "agent #{i}: 候选地址必须具体，got {}",
                    c.address()
                );
                assert_eq!(c.port(), port, "agent #{i}: 候选端口须为宣告端口");
            }
        }
    }

    #[tokio::test]
    async fn build_rtc_setting_engine_always_some() {
        // 三键全缺省也恒返回 SettingEngine（mDNS Disabled 统一在此生效）
        let cfg = Config::default();
        assert!(build_rtc_setting_engine(&cfg).await.is_ok());

        // 显式写回空串/0 同样按未配置处理（仅 mDNS Disabled）
        let cfg = Config {
            rtc_external_ip: "  ".into(),
            ..Default::default()
        };
        assert!(build_rtc_setting_engine(&cfg).await.is_ok());
    }

    #[tokio::test]
    async fn build_rtc_setting_engine_ok_with_ip_only() {
        // 仅外部 IP（host 网络容器等端口本就可达的场景）：不绑定 socket 即可构造
        let cfg = Config {
            rtc_external_ip: "192.168.1.10".into(),
            ..Default::default()
        };
        assert!(build_rtc_setting_engine(&cfg).await.is_ok());
    }

    #[tokio::test]
    async fn build_rtc_setting_engine_some_with_fixed_port() {
        // 固定端口（合法成对配置）：绑定一次成功即可（进程级 OnceCell 共享，
        // 先到先得；重复调用拿到缓存实例）
        let probe = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let free_port = probe.local_addr().unwrap().port();
        drop(probe);
        let cfg = Config {
            rtc_udp_port: free_port,
            rtc_external_ip: "192.0.2.50".into(),
            rtc_external_port: 0,
            ..Default::default()
        };
        assert!(build_rtc_setting_engine(&cfg).await.is_ok());
        assert!(build_rtc_setting_engine(&cfg).await.is_ok());
    }
}
