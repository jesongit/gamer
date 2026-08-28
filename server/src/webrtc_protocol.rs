//! Pure helpers for WebRTC wire-format parsing.
//!
//! `webrtc.rs` owns peer/session orchestration and RTP I/O.  SDP payload type
//! lookup and Annex-B splitting are independent of those concerns, so they
//! live here and can be tested without a peer connection or Android device.

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

#[cfg(test)]
mod tests {
    use super::{
        annexb_nalus, is_h264_config_nal, is_h264_idr_nal, nal_unit_type, payload_type_for,
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
}
