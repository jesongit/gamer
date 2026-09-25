//! Streaming Matroska bridge for clocked BGR24 / Opus packets.
//! Each block has its own cluster, so a stalled video track never alters audio time.
use anyhow::Result;

fn element(id: u32, data: &[u8]) -> Vec<u8> {
    let mut out = id
        .to_be_bytes()
        .into_iter()
        .skip_while(|b| *b == 0)
        .collect::<Vec<_>>();
    let n = data.len() as u64;
    let width = (1..=8).find(|w| n < (1u64 << (7 * w)) - 1).unwrap();
    let size = n | (1u64 << (7 * width));
    out.extend_from_slice(&size.to_be_bytes()[8 - width..]);
    out.extend_from_slice(data);
    out
}
fn uint(id: u32, n: u64) -> Vec<u8> {
    element(id, &n.to_be_bytes())
}
fn group(id: u32, parts: &[Vec<u8>]) -> Vec<u8> {
    element(id, &parts.concat())
}

pub fn raw_header(width: u32, height: u32, audio: bool) -> Result<Vec<u8>> {
    let size = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| anyhow::anyhow!("视频尺寸溢出"))?;
    let mut bitmap = Vec::new();
    bitmap.extend_from_slice(&40u32.to_le_bytes());
    bitmap.extend_from_slice(&width.to_le_bytes());
    bitmap.extend_from_slice(&(-(height as i32)).to_le_bytes());
    bitmap.extend_from_slice(&1u16.to_le_bytes());
    bitmap.extend_from_slice(&24u16.to_le_bytes());
    bitmap.extend_from_slice(&0u32.to_le_bytes());
    bitmap.extend_from_slice(&size.to_le_bytes());
    bitmap.extend_from_slice(&[0; 16]);
    tracks_header(width, height, "V_MS/VFW/FOURCC", &bitmap, audio)
}
fn tracks_header(
    width: u32,
    height: u32,
    codec: &str,
    private: &[u8],
    audio: bool,
) -> Result<Vec<u8>> {
    let mut out = group(
        0x1a45dfa3,
        &[
            uint(0x4286, 1),
            uint(0x42f7, 1),
            uint(0x42f2, 4),
            uint(0x42f3, 8),
            element(0x4282, b"matroska"),
            uint(0x4287, 4),
            uint(0x4285, 2),
        ],
    );
    out.extend_from_slice(&[
        0x18, 0x53, 0x80, 0x67, 0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    ]);
    out.extend(group(
        0x1549a966,
        &[
            uint(0x2ad7b1, 1_000_000),
            element(0x4d80, b"Gamer"),
            element(0x5741, b"Gamer"),
        ],
    ));
    let video = group(
        0xae,
        &[
            uint(0xd7, 1),
            uint(0x73c5, 1),
            uint(0x83, 1),
            element(0x86, codec.as_bytes()),
            element(0x63a2, private),
            group(0xe0, &[uint(0xb0, width as u64), uint(0xba, height as u64)]),
        ],
    );
    let mut tracks = vec![video];
    if audio {
        // Joining an existing live Opus stream needs no encoder-start pre-skip.
        // scrcpy output is fixed at 48 kHz, stereo, channel mapping family zero.
        let mut opus = b"OpusHead".to_vec();
        opus.extend_from_slice(&[1, 2, 0, 0, 0x80, 0xbb, 0, 0, 0, 0, 0]);
        tracks.push(group(
            0xae,
            &[
                uint(0xd7, 2),
                uint(0x73c5, 2),
                uint(0x83, 2),
                element(0x86, b"A_OPUS"),
                element(0x63a2, &opus),
                uint(0x56aa, 0),
                uint(0x56bb, 80_000_000),
                group(
                    0xe1,
                    &[element(0xb5, &48000f64.to_be_bytes()), uint(0x9f, 2)],
                ),
            ],
        ));
    }
    out.extend(group(0x1654ae6b, &tracks));
    Ok(out)
}

pub fn raw_packet(track: u8, pts_us: u64, key: bool, data: &[u8]) -> Result<Vec<u8>> {
    let mut block = vec![0x80 | track, 0, 0, if key { 0x80 } else { 0 }];
    block.extend_from_slice(data);
    Ok(group(
        0x1f43b675,
        &[uint(0xe7, pts_us / 1000), element(0xa3, &block)],
    ))
}
