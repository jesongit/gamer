//! WebRTC 媒体链路的 ICE 候选外部宣告（容器 / NAT 1-to-1 部署适配）。
//!
//! 背景：Docker bridge 部署时服务端 host candidate 是容器内网 IP（172.x），
//! 宿主浏览器无法路由 → 信令（WS）正常但媒体（RTP/UDP）协商不出候选对 →
//! 投屏黑屏（浏览器端一直转圈，icegatheringstate 卡住）。经典 NAT 1-to-1
//! 场景用 config.toml 三个键宣告「宿主可达的外部地址」：
//!
//! - `rtc_external_ip`：候选一律宣告该 IP（`set_nat_1to1_ips(.., Host)` 语义，
//!   不经 STUN/接口枚举，容器内网 IP 不再宣告）；
//! - `rtc_udp_port`：媒体 UDP 换单 socket UDPMux 固定绑该端口（容器端口映射
//!   的前提；0 = 既有行为：每会话临时端口）；
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
//! 实现注记（webrtc-rs 0.13 / webrtc-ice 0.13）：`set_nat_1to1_ips` 只重写候选
//! **IP**，候选**端口**恒取 mux socket 的 `local_addr().port()`（agent_gather.rs
//! `gather_candidates_local_udp_mux`），没有端口重写 API。「绑 A 宣 B」因此经
//! 自定义 [`AdvertisedAddrConn`] 实现：tokio UdpSocket 包一层、`local_addr()`
//! 汇报宣告端口，交给 `UDPMuxDefault`（候选端口取自该值）；收发包全部委托
//! 真实 socket，STUN 应答回源地址，docker DNAT 对链路透明。
//!
//! 三键全缺省 → [`build_rtc_setting_engine`] 返回 None，PeerConnection 构建
//! 路径与既有完全一致（Windows 直跑 / 既有部署零变化）。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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
/// 汇报「对外宣告端口」——webrtc-ice 的 muxed 候选端口取自该值，由此实现
/// 「容器内绑 A、候选宣 B」的端口映射语义。汇报地址固定 0.0.0.0（候选 IP
/// 由 nat1to1 / 接口枚举另行决定，这里只有端口是有效字段；IPv4 家族与
/// `bind 0.0.0.0` 一致，不影响 mux 内部 IPv4-mapped 地址归一化）。
struct AdvertisedAddrConn {
    sock: tokio::net::UdpSocket,
    advertised: SocketAddr,
}

impl AdvertisedAddrConn {
    fn new(sock: tokio::net::UdpSocket, advertised_port: u16) -> Self {
        Self {
            sock,
            advertised: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), advertised_port),
        }
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

async fn shared_udp_mux(cfg: &Config) -> anyhow::Result<Arc<UDPMuxDefault>> {
    SHARED_UDP_MUX
        .get_or_try_init(|| async {
            let sock = tokio::net::UdpSocket::bind(("0.0.0.0", cfg.rtc_udp_port))
                .await
                .map_err(|e| {
                    anyhow::anyhow!("rtc_udp_port={} UDP 绑定失败：{e}", cfg.rtc_udp_port)
                })?;
            let conn = AdvertisedAddrConn::new(
                sock,
                advertised_port(cfg.rtc_udp_port, cfg.rtc_external_port),
            );
            Ok::<_, anyhow::Error>(UDPMuxDefault::new(UDPMuxParams::new(conn)))
        })
        .await
        .cloned()
}

/// 按 rtc_* 配置构造 SettingEngine；三键全缺省返回 None（调用方保持既有
/// 无 SettingEngine 的构建路径，行为零变化）。
///
/// - `rtc_external_ip` 非空：`set_nat_1to1_ips(.., Host)`——host 候选一律
///   宣告该 IP。webrtc-rs 默认 mDNS 为 QueryOnly，不会触发 nat1to1×mDNS
///   冲突（仅 QueryAndGather 报错）；ICE_SERVERS 的 STUN 不受影响。
/// - `rtc_udp_port` 非 0：媒体 UDP 换单 socket UDPMux 固定绑端口（进程级
///   共享，见 [`shared_udp_mux`]）；候选端口 = 绑定端口或 [`advertised_port`]
///   的对外映射端口。
pub(crate) async fn build_rtc_setting_engine(
    cfg: &Config,
) -> anyhow::Result<Option<SettingEngine>> {
    let nat_ip = cfg.rtc_external_ip.trim();
    if nat_ip.is_empty() && cfg.rtc_udp_port == 0 {
        return Ok(None);
    }

    let mut se = SettingEngine::default();
    if !nat_ip.is_empty() {
        se.set_nat_1to1_ips(vec![nat_ip.to_string()], RTCIceCandidateType::Host);
    }
    if cfg.rtc_udp_port != 0 {
        let mux = shared_udp_mux(cfg).await?;
        se.set_udp_network(UDPNetwork::Muxed(mux));
    }
    Ok(Some(se))
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
    async fn advertised_addr_conn_delegates_io_and_reports_advertised_port() {
        // 真实 socket 走临时端口（不依赖固定端口可用性），宣告端口随便指定
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let real_port = sock.local_addr().unwrap().port();
        let conn = AdvertisedAddrConn::new(sock, 40001);

        // local_addr 被改写为宣告端口（IP 家族保持 IPv4），与真实绑定端口无关
        let local = conn.local_addr().unwrap();
        assert_eq!(local.port(), 40001);
        assert!(local.is_ipv4());
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

    #[tokio::test]
    async fn build_rtc_setting_engine_none_when_unconfigured() {
        // 三键全缺省：不构造 SettingEngine（既有路径零变化的锚点）
        let cfg = Config::default();
        assert!(build_rtc_setting_engine(&cfg).await.unwrap().is_none());

        // 显式写回空串/0 同样按未配置处理
        let cfg = Config {
            rtc_external_ip: "  ".into(),
            ..Default::default()
        };
        assert!(build_rtc_setting_engine(&cfg).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn build_rtc_setting_engine_some_with_ip_only() {
        // 仅外部 IP（host 网络容器等多播场景）：不绑定 socket 即可构造
        let cfg = Config {
            rtc_external_ip: "192.168.1.10".into(),
            ..Default::default()
        };
        assert!(build_rtc_setting_engine(&cfg).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn build_rtc_setting_engine_some_with_fixed_port() {
        // 固定端口：绑定一次成功即可（进程级 OnceCell 共享，先到先得；
        // 重复调用拿到缓存实例，返回值仍为 Some）
        let probe = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let free_port = probe.local_addr().unwrap().port();
        drop(probe);
        let cfg = Config {
            rtc_udp_port: free_port,
            rtc_external_port: 0,
            ..Default::default()
        };
        assert!(build_rtc_setting_engine(&cfg).await.unwrap().is_some());
        assert!(build_rtc_setting_engine(&cfg).await.unwrap().is_some());
    }
}
