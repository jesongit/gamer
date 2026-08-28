//! Pure helpers for WebRTC wire-format parsing.
//!
//! `webrtc.rs` owns peer/session orchestration and RTP I/O.  SDP payload type
//! lookup and Annex-B splitting are independent of those concerns, so they
//! live here and can be tested without a peer connection or Android device.

use bytes::Bytes;
use webrtc::rtp::header::Header;
use webrtc::rtp::packet::Packet;

/// Return the first payload type whose `rtpmap` line contains `encoding`.
pub(crate) fn payload_type_for(sdp: &str, encoding: &str) -> Option<u8> {
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            if rest.contains(encoding) {
                if let Some(pt) = rest.split_whitespace().next().and_then(|s| s.parse().ok()) {
                    return Some(pt);
                }
            }
        }
    }
    None
}

/// Build one RTP packet per SPS/PPS NALU and keep them on the same timestamp.
///
/// The caller owns `seq` so the packet sequence advances exactly as the real
/// sender would.
#[allow(dead_code)]
pub(crate) fn build_config_packets(
    cfg: &[u8],
    payload_type: u8,
    ssrc: u32,
    seq: &mut u16,
    ts: u32,
) -> Vec<Packet> {
    annexb_nalus(cfg)
        .into_iter()
        .filter(|nal| matches!(nal_unit_type(nal), Some(7 | 8)) && nal.len() <= 1200)
        .map(|nal| {
            let pkt = Packet {
                header: Header {
                    version: 2,
                    padding: false,
                    extension: false,
                    marker: false,
                    payload_type,
                    sequence_number: *seq,
                    timestamp: ts,
                    ssrc,
                    ..Default::default()
                },
                payload: Bytes::copy_from_slice(nal),
            };
            *seq = seq.wrapping_add(1);
            pkt
        })
        .collect()
}

/// Rebuild the original H.264 NAL stream from RTP payloads.
///
/// The output keeps the payload NAL type so tests can assert on SPS/PPS, IDR
/// and FU-A reconstruction without a full decoder.
#[allow(dead_code)]
pub(crate) fn rebuild_h264_nalus(payloads: &[Bytes]) -> Vec<(u8, Bytes)> {
    let mut nals: Vec<(u8, Bytes)> = Vec::new();
    for p in payloads {
        if p.is_empty() {
            continue;
        }
        let t = p[0] & 0x1F;
        match t {
            24 => {
                let mut off = 1usize;
                while off + 2 <= p.len() {
                    let len = ((p[off] as usize) << 8) | p[off + 1] as usize;
                    off += 2;
                    if off + len > p.len() {
                        break;
                    }
                    let n = p.slice(off..off + len);
                    if !n.is_empty() {
                        nals.push((n[0] & 0x1F, n));
                    }
                    off += len;
                }
            }
            28 | 29 => {
                let start = p[1] & 0x80 != 0;
                let typ = p[1] & 0x1F;
                let data = p.slice(2..);
                if start {
                    let nri = p[0] & 0x60;
                    let mut nal = Vec::with_capacity(data.len() + 1);
                    nal.push(nri | typ);
                    nal.extend_from_slice(&data);
                    nals.push((typ, Bytes::from(nal)));
                } else if let Some((_, last)) = nals.last_mut() {
                    let mut merged = last.to_vec();
                    merged.extend_from_slice(&data);
                    *last = Bytes::from(merged);
                }
            }
            1..=23 => nals.push((t, p.clone())),
            _ => {}
        }
    }
    nals
}

/// Return the H.264 NAL unit type encoded in the first byte.
#[allow(dead_code)]
pub(crate) fn nal_unit_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| b & 0x1F)
}

#[allow(dead_code)]
pub(crate) fn is_h264_config_nal(nal: &[u8]) -> bool {
    matches!(nal_unit_type(nal), Some(7 | 8))
}

#[allow(dead_code)]
pub(crate) fn is_h264_idr_nal(nal: &[u8]) -> bool {
    matches!(nal_unit_type(nal), Some(5))
}

/// Split an Annex-B byte stream into NAL units.
///
/// Bytes before a start code are ignored, matching the existing WebRTC
/// sender behavior. Both three- and four-byte start codes are accepted.
pub(crate) fn annexb_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let start_code_len = if pos + 4 <= data.len() && data[pos..pos + 4] == [0, 0, 0, 1] {
            4
        } else if pos + 3 <= data.len() && data[pos..pos + 3] == [0, 0, 1] {
            3
        } else {
            0
        };
        if start_code_len == 0 {
            pos += 1;
            continue;
        }

        let nal_start = pos + start_code_len;
        let mut nal_end = data.len();
        let mut zero_count = 0usize;
        for (index, byte) in data.iter().enumerate().skip(nal_start) {
            if *byte == 0 {
                zero_count += 1;
                continue;
            }
            if *byte == 1 && zero_count >= 2 {
                nal_end = index - zero_count;
                break;
            }
            zero_count = 0;
        }
        if nal_end > nal_start {
            nals.push(&data[nal_start..nal_end]);
        }
        pos = nal_end;
    }
    nals
}

/// Return NAL units together with their types, skipping AUD/FILLER.
#[allow(dead_code)]
pub(crate) fn annexb_nalus_with_types(data: &[u8]) -> Vec<(u8, Vec<u8>)> {
    annexb_nalus(data)
        .into_iter()
        .filter_map(|nal| {
            let t = nal_unit_type(nal)?;
            if t == 9 || t == 12 {
                None
            } else {
                Some((t, nal.to_vec()))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{
        annexb_nalus, annexb_nalus_with_types, build_config_packets, is_h264_config_nal,
        is_h264_idr_nal, nal_unit_type, payload_type_for, rebuild_h264_nalus,
    };

    #[test]
    fn payload_type_for_returns_first_matching_rtpmap() {
        let sdp = "a=rtpmap:111 opus/48000/2\r\na=rtpmap:102 H264/90000\r\na=rtpmap:127 H264/90000";

        assert_eq!(payload_type_for(sdp, "H264/90000"), Some(102));
        assert_eq!(payload_type_for(sdp, "opus/48000"), Some(111));
    }

    #[test]
    fn payload_type_for_ignores_non_rtpmap_and_invalid_payload_types() {
        let sdp = "a=fmtp:102 H264/90000\n a=rtpmap:nope H264/90000\n a=rtpmap:abc opus/48000";

        assert_eq!(payload_type_for(sdp, "H264/90000"), None);
        assert_eq!(payload_type_for(sdp, "opus/48000"), None);
    }

    #[test]
    fn annexb_nalus_accepts_three_and_four_byte_start_codes() {
        let data = [
            0x55, 0x66, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE, 0x3C,
        ];

        assert_eq!(annexb_nalus(&data), vec![&data[5..7], &data[11..]]);
    }

    #[test]
    fn annexb_nalus_skips_empty_units_and_preserves_nal_bytes() {
        let data = [0, 0, 1, 0, 0, 0, 1, 0x65, 0x01, 0, 0, 1, 0x41];

        assert_eq!(annexb_nalus(&data), vec![&data[7..9], &data[12..]]);
    }

    #[test]
    fn annexb_nalus_returns_empty_for_non_annex_b_data() {
        assert!(annexb_nalus(&[0x67, 0x42, 0x00]).is_empty());
    }

    #[test]
    fn nal_helpers_classify_config_and_idr_units() {
        assert_eq!(nal_unit_type(&[0x67, 0x00]), Some(7));
        assert!(is_h264_config_nal(&[0x67, 0x00]));
        assert!(is_h264_config_nal(&[0x68]));
        assert!(is_h264_idr_nal(&[0x65, 0x00]));
        assert!(!is_h264_idr_nal(&[0x61, 0x00]));
    }

    #[test]
    fn annexb_nalus_with_types_keeps_h264_reference_nals() {
        let data = [
            0, 0, 0, 1, 0x09, 0x10, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE, 0x3C,
        ];

        assert_eq!(
            annexb_nalus_with_types(&data),
            vec![(7, vec![0x67, 0x42]), (8, vec![0x68, 0xCE, 0x3C])]
        );
    }

    #[test]
    fn rebuild_h264_nalus_reconstructs_fu_a_and_stap_a_payloads() {
        let payloads = vec![
            Bytes::from_static(&[24, 0, 2, 0x67, 0x64, 0, 3, 0x68, 0xAA, 0xBB]),
            Bytes::from_static(&[0x7C, 0x85, 0x11, 0x22, 0x23]),
            Bytes::from_static(&[0x7C, 0x05, 0x33, 0x44]),
        ];

        let rebuilt = rebuild_h264_nalus(&payloads);
        assert_eq!(
            rebuilt,
            vec![
                (7, Bytes::from_static(&[0x67, 0x64])),
                (8, Bytes::from_static(&[0x68, 0xAA, 0xBB])),
                (5, Bytes::from_static(&[0x65, 0x11, 0x22, 0x23, 0x33, 0x44])),
            ]
        );
    }

    #[test]
    fn build_config_packets_uses_same_timestamp_and_marker_false() {
        let cfg = [
            0, 0, 0, 1, 0x67, 0x64, 0x00, 0x32, 0, 0, 0, 1, 0x68, 0xE9, 0x70, 0x4C, 0xB2, 0x2C,
        ];
        let mut seq = 7u16;

        let packets = build_config_packets(&cfg, 96, 4242, &mut seq, 9000);
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].header.sequence_number, 7);
        assert_eq!(packets[1].header.sequence_number, 8);
        assert!(!packets[0].header.marker);
        assert!(!packets[1].header.marker);
        assert_eq!(packets[0].header.timestamp, 9000);
        assert_eq!(packets[1].header.timestamp, 9000);
        assert_eq!(packets[0].header.payload_type, 96);
        assert_eq!(packets[0].header.ssrc, 4242);
        assert_eq!(seq, 9);
        assert_eq!(packets[0].payload.as_ref()[0] & 0x1F, 7);
        assert_eq!(packets[1].payload.as_ref()[0] & 0x1F, 8);
    }
}
