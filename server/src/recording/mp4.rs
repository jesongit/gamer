//! 最小 H.264 MP4（ISO BMFF）封装器：单视频轨、样本级精确 PTS。
//!
//! 为什么不用 `ffmpeg -c copy` remux 原始 Annex-B（对实施合同 §2 的偏离，
//! 已申报）：raw h264 输入端口没有时间戳，ffmpeg 只能按假设帧率生成容器
//! 时间戳——「事件时间轴 ↔ 画面精确帧」的同步（视频工作台的核心能力）会被
//! 系统性破坏。这里直接按 ISO BMFF 顺序写 `ftyp + mdat(样本 AVCC 化) + moov`，
//! PTS 全程保留 scrcpy 原始媒体时钟（归一化到段起点，timescale = 1 MHz µs），
//! 不做重编码（样本字节原样进入 mdat，等价于 `-c copy`）。
//!
//! 结构：mdat 先流式写入（崩溃只丢当前段，此前已收口的段完好），moov 在
//! finish 时一次性补写；sha256/字节量在写入时增量计算，finalize 无二次读盘。
//! 结构合法性由单测锁定（顶层 box 遍历 + 样本表一致性），真实验证走
//! `#[ignore]` 的 ffprobe 测试（需外部 ffmpeg，见 ignored_playback_smoke）。

use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

/// 时间片（µs）：PTS 原样进容器，不引入假设帧率。
const TIMESCALE: u32 = 1_000_000;
/// 单段 mdat 硬上限（u32 box size 之内留余量）：触顶返回错误，由录制层按
/// 磁盘压力收口当前段并分段。
pub const MAX_MDAT_BYTES: u64 = u32::MAX as u64 - 64 * 1024 * 1024;

/// finalize 产物摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxSummary {
    pub size_bytes: u64,
    pub sha256: String,
    pub duration_us: u64,
    pub samples: u32,
}

/// Annex-B → MP4 流式封装器（一个实例 = 一段视频）。
pub struct H264Mp4Writer {
    file: io::BufWriter<std::fs::File>,
    offset: u64,
    hasher: Sha256,
    mdat_size_pos: u64,
    mdat_data_start: u64,
    mdat_bytes: u64,
    sps: Vec<u8>,
    pps: Vec<u8>,
    width: u32,
    height: u32,
    sizes: Vec<u32>,
    /// stts 运行长度：(sample_count, delta_us)，按样本顺序。
    stts: Vec<(u32, u32)>,
    syncs: Vec<u32>,
    last_pts: Option<u64>,
    /// 最后一个已压入的 delta（尾样本缺 delta 时复用）。
    last_delta: u32,
    sum_delta: u64,
    samples: u32,
    finished: bool,
}

impl H264Mp4Writer {
    pub fn create(path: &Path, width: u32, height: u32) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut w = Self {
            file: io::BufWriter::new(std::fs::File::create(path)?),
            offset: 0,
            hasher: Sha256::new(),
            mdat_size_pos: 0,
            mdat_data_start: 0,
            mdat_bytes: 0,
            sps: Vec::new(),
            pps: Vec::new(),
            width,
            height,
            sizes: Vec::new(),
            stts: Vec::new(),
            syncs: Vec::new(),
            last_pts: None,
            last_delta: 0,
            sum_delta: 0,
            samples: 0,
            finished: false,
        };
        w.write_hashed(&box_bytes(b"ftyp", &ftyp_payload()))?;
        w.mdat_size_pos = w.offset;
        w.write_hashed(&0u32.to_be_bytes())?;
        w.write_hashed(b"mdat")?;
        w.mdat_data_start = w.offset;
        Ok(w)
    }

    /// 喂入独立的参数集包（scrcpy is_config 帧：SPS/PPS Annex-B）。
    /// 参数集不进 mdat（AVCC 化后进 avcC）；段内参数集变化由录制层负责分段。
    pub fn set_config(&mut self, annexb: &[u8]) {
        for nal in split_annexb(annexb) {
            match nal_type(nal) {
                7 => self.sps = nal.to_vec(),
                8 => self.pps = nal.to_vec(),
                _ => {}
            }
        }
    }

    /// 是否已捕获有效参数集（开段前置条件）。
    pub fn has_config(&self) -> bool {
        !self.sps.is_empty() && !self.pps.is_empty()
    }

    /// 写入一个访问单元（Annex-B，一帧一样本），pts 为段内相对时间（µs）。
    /// 返回写入 mdat 的字节数（纯参数集包返回 0）。
    pub fn write_annexb_sample(&mut self, au: &[u8], pts_us: u64, keyframe: bool) -> io::Result<usize> {
        let mut avcc = Vec::with_capacity(au.len() + 16);
        for nal in split_annexb(au) {
            match nal_type(nal) {
                7 => {
                    self.sps = nal.to_vec();
                }
                8 => {
                    self.pps = nal.to_vec();
                }
                _ => {
                    avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    avcc.extend_from_slice(nal);
                }
            }
        }
        if avcc.is_empty() {
            return Ok(0);
        }
        // stts：sample i 的 delta = pts[i+1] - pts[i]；首样本的 delta 在次样本
        // 到达时补记；PTS 回退/重复钳为 0（不产生负 delta，ISO 规定非负）。
        match self.last_pts {
            None => {
                self.last_pts = Some(pts_us);
            }
            Some(prev) => {
                let delta = pts_us.saturating_sub(prev).min(u32::MAX as u64) as u32;
                self.push_delta(delta);
                self.last_pts = Some(pts_us);
            }
        }
        if self.mdat_bytes + avcc.len() as u64 > MAX_MDAT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "segment exceeds MP4 mdat size limit",
            ));
        }
        self.write_hashed(&avcc)?;
        self.mdat_bytes += avcc.len() as u64;
        self.sizes.push(avcc.len() as u32);
        if keyframe {
            self.syncs.push(self.samples + 1); // 1-based sample number
        }
        self.samples += 1;
        Ok(avcc.len())
    }

    fn push_delta(&mut self, delta: u32) {
        self.sum_delta += delta as u64;
        self.last_delta = delta;
        match self.stts.last_mut() {
            Some((count, d)) if *d == delta => *count += 1,
            _ => self.stts.push((1, delta)),
        }
    }

    /// 收口：补写 moov、回填 mdat 长度、flush。消费 self；产物不可再写。
    pub fn finish(mut self) -> io::Result<MuxSummary> {
        if self.finished {
            return Err(io::Error::other("muxer already finished"));
        }
        if !self.has_config() {
            return Err(io::Error::other("missing SPS/PPS (no decodable start point)"));
        }
        if self.samples == 0 {
            return Err(io::Error::other("no samples written"));
        }
        // 尾样本的 delta：复用最后一个 delta（单样本段为 0）。
        self.push_delta(self.last_delta);
        let duration_us = self.sum_delta;

        let moov = self.build_moov(duration_us);
        // 回填 mdat 长度（size 字段含 8 字节 box 头）。
        self.flush()?;
        let file = self.file.get_mut();
        file.seek(SeekFrom::Start(self.mdat_size_pos))?;
        file.write_all(&((self.mdat_bytes + 8) as u32).to_be_bytes())?;
        file.seek(SeekFrom::Start(self.offset))?;
        self.write_hashed(&moov)?;
        self.flush()?;
        self.finished = true;
        Ok(MuxSummary {
            size_bytes: self.offset,
            sha256: hex(&self.hasher.clone().finalize()),
            duration_us,
            samples: self.samples,
        })
    }

    fn build_moov(&self, duration_us: u64) -> Vec<u8> {
        let duration_ms = duration_us.div_ceil(1000);
        let matrix: [u32; 9] = [0x00010000, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000];

        // mvhd（version 0，movie timescale = 1000 ms）
        let mut mvhd = full_header(0, 0);
        mvhd.extend(0u32.to_be_bytes()); // creation
        mvhd.extend(0u32.to_be_bytes()); // modification
        mvhd.extend(1000u32.to_be_bytes());
        mvhd.extend((duration_ms as u32).to_be_bytes());
        mvhd.extend(0x00010000u32.to_be_bytes()); // rate
        mvhd.extend(0x0100u16.to_be_bytes()); // volume
        mvhd.extend(0u16.to_be_bytes()); // reserved
        mvhd.extend(0u32.to_be_bytes());
        mvhd.extend(0u32.to_be_bytes()); // reserved(8)
        for m in matrix {
            mvhd.extend(m.to_be_bytes());
        }
        mvhd.extend([0u8; 24]); // pre_defined
        mvhd.extend(2u32.to_be_bytes()); // next_track_ID

        // tkhd（version 0，flags = enabled | in_movie | in_preview）
        let mut tkhd = full_header(0, 0x000007);
        tkhd.extend(0u32.to_be_bytes());
        tkhd.extend(0u32.to_be_bytes());
        tkhd.extend(1u32.to_be_bytes()); // track_ID
        tkhd.extend(0u32.to_be_bytes()); // reserved
        tkhd.extend((duration_ms as u32).to_be_bytes());
        tkhd.extend([0u8; 8]); // reserved
        tkhd.extend(0u16.to_be_bytes()); // layer
        tkhd.extend(0u16.to_be_bytes()); // alternate_group
        tkhd.extend(0u16.to_be_bytes()); // volume（视频轨 0）
        tkhd.extend(0u16.to_be_bytes()); // reserved
        for m in matrix {
            tkhd.extend(m.to_be_bytes());
        }
        tkhd.extend((self.width << 16).to_be_bytes()); // 16.16 fixed
        tkhd.extend((self.height << 16).to_be_bytes());

        // mdhd（version 1：64-bit duration，µs > 71 分钟也安全）
        let mut mdhd = full_header(1, 0);
        mdhd.extend(0u64.to_be_bytes());
        mdhd.extend(0u64.to_be_bytes());
        mdhd.extend(TIMESCALE.to_be_bytes());
        mdhd.extend(duration_us.to_be_bytes());
        mdhd.extend(0x55C4u16.to_be_bytes()); // language "und"
        mdhd.extend(0u16.to_be_bytes()); // pre_defined

        let mut hdlr = full_header(0, 0);
        hdlr.extend(0u32.to_be_bytes()); // pre_defined
        hdlr.extend(b"vide");
        hdlr.extend([0u8; 12]);
        hdlr.extend(b"VideoHandler\0");

        let mut vmhd = full_header(0, 1);
        vmhd.extend(0u16.to_be_bytes()); // graphicsmode
        vmhd.extend([0u8; 6]); // opcolor

        // dinf/dref/url：flags=1（self-contained，数据就在本文件 mdat）。
        // 注意 dinf 包装必须有——mov demuxer 只认 minf > dinf > dref 链。
        let url_box = box_bytes(b"url ", &full_header(0, 1));
        let dref_payload = push_all(push_all(full_header(0, 0), &1u32.to_be_bytes()), &url_box);
        let dref = box_bytes(b"dref", &dref_payload);
        let dinf = box_bytes(b"dinf", &dref);

        let avcc = build_avcc(&self.sps, &self.pps);
        let mut avc1 = Vec::with_capacity(78 + avcc.len());
        avc1.extend([0u8; 6]); // reserved
        avc1.extend(1u16.to_be_bytes()); // data_reference_index
        avc1.extend([0u8; 16]); // pre_defined + reserved
        avc1.extend((self.width as u16).to_be_bytes());
        avc1.extend((self.height as u16).to_be_bytes());
        avc1.extend(0x00480000u32.to_be_bytes()); // horiz res 72dpi
        avc1.extend(0x00480000u32.to_be_bytes());
        avc1.extend(0u32.to_be_bytes()); // reserved
        avc1.extend(1u16.to_be_bytes()); // frame_count
        avc1.extend([0u8; 32]); // compressorname
        avc1.extend(0x0018u16.to_be_bytes()); // depth
        avc1.extend(0xFFFFu16.to_be_bytes()); // pre_defined = -1
        avc1.extend(box_bytes(b"avcC", &avcc));

        let stsd_payload = push_all(
            push_all(full_header(0, 0), &1u32.to_be_bytes()),
            &box_bytes(b"avc1", &avc1),
        );
        let stsd = box_bytes(b"stsd", &stsd_payload);

        let mut stts_payload = full_header(0, 0);
        stts_payload.extend((self.stts.len() as u32).to_be_bytes());
        for (count, delta) in &self.stts {
            stts_payload.extend(count.to_be_bytes());
            stts_payload.extend(delta.to_be_bytes());
        }

        let mut stbl = Vec::new();
        stbl.extend(stsd);
        stbl.extend(box_bytes(b"stts", &stts_payload));
        // stss：全部样本都是关键帧时省略（等价 all-sync）。
        if self.syncs.len() < self.samples as usize {
            let mut stss = full_header(0, 0);
            stss.extend((self.syncs.len() as u32).to_be_bytes());
            for s in &self.syncs {
                stss.extend(s.to_be_bytes());
            }
            stbl.extend(box_bytes(b"stss", &stss));
        }
        let mut stsc = full_header(0, 0);
        stsc.extend(1u32.to_be_bytes()); // entry_count
        stsc.extend(1u32.to_be_bytes()); // first_chunk
        stsc.extend(self.samples.to_be_bytes()); // samples_per_chunk
        stsc.extend(1u32.to_be_bytes()); // sample_description_index
        stbl.extend(box_bytes(b"stsc", &stsc));

        let mut stsz = full_header(0, 0);
        stsz.extend(0u32.to_be_bytes()); // sample_size（非定长）
        stsz.extend(self.samples.to_be_bytes());
        for s in &self.sizes {
            stsz.extend(s.to_be_bytes());
        }
        stbl.extend(box_bytes(b"stsz", &stsz));

        let mut stco = full_header(0, 0);
        stco.extend(1u32.to_be_bytes());
        stco.extend((self.mdat_data_start as u32).to_be_bytes());
        stbl.extend(box_bytes(b"stco", &stco));

        let mut minf = Vec::new();
        minf.extend(box_bytes(b"vmhd", &vmhd));
        minf.extend(dinf);
        minf.extend(box_bytes(b"stbl", &stbl));

        let mut mdia = Vec::new();
        mdia.extend(box_bytes(b"mdhd", &mdhd));
        mdia.extend(box_bytes(b"hdlr", &hdlr));
        mdia.extend(box_bytes(b"minf", &minf));

        let trak = box_bytes(b"trak", &push_all(
            box_bytes(b"tkhd", &tkhd),
            &box_bytes(b"mdia", &mdia),
        ));
        box_bytes(b"moov", &push_all(box_bytes(b"mvhd", &mvhd), &trak))
    }

    fn write_hashed(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.write_all(bytes)?;
        self.hasher.update(bytes);
        self.offset += bytes.len() as u64;
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

// ---------- Annex-B 解析 ----------

/// 切分 Annex-B 为 NAL 单元（去 start code；剥离 trailing zero）。
pub(crate) fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut positions = Vec::new();
    let mut i = 0usize;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            positions.push(i);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(positions.len());
    for (k, &pos) in positions.iter().enumerate() {
        let start = pos + 3;
        let mut end = if k + 1 < positions.len() {
            positions[k + 1]
        } else {
            data.len()
        };
        // 去掉归属下一个 start code / trailing_zero_8bits 的 0x00。
        while end > start && data[end - 1] == 0 {
            end -= 1;
        }
        if end > start {
            nals.push(&data[start..end]);
        }
    }
    nals
}

/// NAL header 的 nal_unit_type（低 5 位）。
pub(crate) fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map(|b| b & 0x1F).unwrap_or(0)
}

/// 访问单元是否内联携带参数集（SPS/PPS）。
pub(crate) fn au_contains_param_set(au: &[u8]) -> bool {
    split_annexb(au)
        .iter()
        .any(|nal| matches!(nal_type(nal), 7 | 8))
}

// ---------- box 构造 ----------

fn full_header(version: u8, flags: u32) -> Vec<u8> {
    vec![version, (flags >> 16) as u8, (flags >> 8) as u8, flags as u8]
}

fn push_all(mut base: Vec<u8>, extra: &[u8]) -> Vec<u8> {
    base.extend_from_slice(extra);
    base
}

fn box_bytes(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len());
    v.extend(((payload.len() + 8) as u32).to_be_bytes());
    v.extend(kind);
    v.extend(payload);
    v
}

fn ftyp_payload() -> Vec<u8> {
    let mut v = Vec::with_capacity(24);
    v.extend(b"isom"); // major brand
    v.extend(512u32.to_be_bytes()); // minor version
    v.extend(b"isom");
    v.extend(b"iso2");
    v.extend(b"avc1");
    v.extend(b"mp41");
    v
}

/// avcC（AVCDecoderConfigurationRecord）：SPS/PPS 打包为 extradata。
fn build_avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(11 + sps.len() + pps.len());
    v.push(1); // configurationVersion
    v.push(*sps.get(1).unwrap_or(&0)); // profile
    v.push(*sps.get(2).unwrap_or(&0)); // compat
    v.push(*sps.get(3).unwrap_or(&0)); // level
    v.push(0xFF); // lengthSizeMinusOne = 3（4 字节长度前缀）
    v.push(0xE1); // numOfSequenceParameterSets = 1
    v.extend((sps.len() as u16).to_be_bytes());
    v.extend(sps);
    v.push(1); // numOfPictureParameterSets
    v.extend((pps.len() as u16).to_be_bytes());
    v.extend(pps);
    v
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合成 NAL：header + payload（不做真实 RBSP）。
    fn nal(t: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![t];
        v.extend(payload);
        v
    }

    fn annexb(nals: &[&[u8]]) -> Vec<u8> {
        let mut v = Vec::new();
        for n in nals {
            v.extend([0, 0, 0, 1]);
            v.extend(*n);
        }
        v
    }

    /// 遍历顶层 box：（fourcc, payload 区间）。
    fn walk_top(buf: &[u8]) -> Vec<(String, (usize, usize))> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while i + 8 <= buf.len() {
            let size = u32::from_be_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
            assert!(size >= 8 && i + size <= buf.len(), "bad box size {size} at {i}");
            let kind = String::from_utf8_lossy(&buf[i + 4..i + 8]).to_string();
            out.push((kind, (i + 8, i + size)));
            i += size;
        }
        assert_eq!(i, buf.len(), "box 尺寸之和必须等于文件长度");
        out
    }

    /// 递归查找 box（moov/trak/mdia/stbl 层级扁平搜索）。
    fn walk_nested(buf: &[u8], range: (usize, usize)) -> Vec<(String, (usize, usize))> {
        let mut out = Vec::new();
        let mut i = range.0;
        while i + 8 <= range.1 {
            let size = u32::from_be_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
            if size < 8 || i + size > range.1 {
                break;
            }
            let kind = String::from_utf8_lossy(&buf[i + 4..i + 8]).to_string();
            out.push((kind, (i + 8, i + size)));
            i += size;
        }
        out
    }

    #[test]
    fn annexb_split_handles_three_and_four_byte_start_codes() {
        let data = annexb(&[&nal(7, &[1, 2]), &nal(8, &[3]), &nal(5, &[4, 5])]);
        let nals = split_annexb(&data);
        assert_eq!(nals.len(), 3);
        assert_eq!(nal_type(nals[0]), 7);
        assert_eq!(nals[0], &[7, 1, 2]);
        assert_eq!(nals[1], &[8, 3]);
        assert_eq!(nals[2], &[5, 4, 5]);
        // 无 start code / 空 → 空
        assert!(split_annexb(b"garbage").is_empty());
        assert!(split_annexb(&[]).is_empty());
        // 尾部 zero 剥离
        let mut with_zeros = annexb(&[&nal(5, &[9])]);
        with_zeros.extend([0, 0, 0]);
        assert_eq!(split_annexb(&with_zeros), vec![&[5, 9][..]]);
    }

    #[test]
    fn mux_structure_is_consistent_and_lossless() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-mp4-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("seg.mp4");

        let sps = nal(7, &[0x67, 0x64, 0x00, 0x1f, 0xac]);
        let pps = nal(8, &[0x68, 0xeb, 0xec, 0x22]);
        let mut w = H264Mp4Writer::create(&path, 192, 108).unwrap();
        w.set_config(&annexb(&[&sps, &pps]));

        // 5 样本：pts 0/33/67/100/133 ms，第 0、3 帧为关键帧；样本内嵌 SEI(6) 不丢
        let pts = [0u64, 33_333, 66_666, 100_000, 133_333];
        for (i, p) in pts.iter().enumerate() {
            let au = if i == 0 {
                annexb(&[&sps, &pps, &nal(5, &[0xAA]), &nal(6, &[0x01])])
            } else if i == 3 {
                annexb(&[&nal(5, &[0xBB])])
            } else {
                annexb(&[&nal(1, &[i as u8, 0x11])])
            };
            let n = w.write_annexb_sample(&au, *p, i == 0 || i == 3).unwrap();
            assert!(n > 0);
        }
        let summary = w.finish().unwrap();
        assert_eq!(summary.samples, 5);
        assert_eq!(summary.duration_us, 133_333 + 33_333); // 尾样本复用上一 delta
        assert_eq!(summary.sha256.len(), 64);

        let file = std::fs::read(&path).unwrap();
        assert_eq!(file.len() as u64, summary.size_bytes, "size 与实际文件一致");

        let top = walk_top(&file);
        assert_eq!(top.len(), 3, "顶层 = ftyp + mdat + moov");
        assert_eq!(top[0].0, "ftyp");
        assert_eq!(top[1].0, "mdat");
        assert_eq!(top[2].0, "moov");

        // mdat：size 占位回填正确、数据 = 5 个 (len+NAL) 样本
        let (mdat_s, mdat_e) = top[1].1;
        let mdat = &file[mdat_s..mdat_e];

        // moov 层级
        let (moov_s, moov_e) = top[2].1;
        let moov = walk_nested(&file, (moov_s, moov_e));
        let find_in = |boxes: &[(String, (usize, usize))], k: &str| {
            boxes.iter().find(|(name, _)| name == k).unwrap_or_else(|| panic!("missing {k}")).1
        };
        let trak = find_in(&moov, "trak");
        let trak_boxes = walk_nested(&file, trak);
        let mdia = find_in(&trak_boxes, "mdia");
        let mdia_boxes = walk_nested(&file, mdia);
        let minf = find_in(&mdia_boxes, "minf");
        let minf_boxes = walk_nested(&file, minf);
        let stbl = find_in(&minf_boxes, "stbl");
        let stbl_boxes = walk_nested(&file, stbl);

        // stsz：count=5，sum(sizes) == mdat payload 长度
        let stsz_r = find_in(&stbl_boxes, "stsz");
        let stsz = &file[stsz_r.0..stsz_r.1];
        assert_eq!(u32::from_be_bytes(stsz[4..8].try_into().unwrap()), 0, "sample_size=0");
        let count = u32::from_be_bytes(stsz[8..12].try_into().unwrap());
        assert_eq!(count, 5);
        let mut total = 0u64;
        for i in 0..count as usize {
            total += u32::from_be_bytes(stsz[12 + i * 4..16 + i * 4].try_into().unwrap()) as u64;
        }
        assert_eq!(total, mdat.len() as u64, "样本表与 mdat 一致");
        // mdat 占位 size 回填 = payload + 8
        let mdat_size = u32::from_be_bytes(file[top[1].1.0 - 8..top[1].1.0 - 4].try_into().unwrap());
        assert_eq!(mdat_size as u64, mdat.len() as u64 + 8);

        // stts：0/33333/33334/33333/33333
        let stts_r = find_in(&stbl_boxes, "stts");
        let stts = &file[stts_r.0..stts_r.1];
        let entries = u32::from_be_bytes(stts[4..8].try_into().unwrap());
        let mut covered = 0u64;
        let mut deltas = Vec::new();
        for i in 0..entries as usize {
            let off = 8 + i * 8;
            let c = u32::from_be_bytes(stts[off..off + 4].try_into().unwrap());
            let d = u32::from_be_bytes(stts[off + 4..off + 8].try_into().unwrap());
            covered += c as u64;
            deltas.push((c, d));
        }
        assert_eq!(covered, 5, "stts 必须覆盖全部样本");
        // stts 运行长度：deltas = [33333, 33333, 33334, 33333, 33333]
        assert_eq!(deltas, vec![(2, 33333), (1, 33334), (2, 33333)]);
        // mdhd duration = sum(deltas)
        let mdhd_r = find_in(&mdia_boxes, "mdhd");
        let mdhd = &file[mdhd_r.0..mdhd_r.1];
        assert_eq!(mdhd[0], 1, "mdhd version 1（64-bit duration）");
        let dur = u64::from_be_bytes(mdhd[24..32].try_into().unwrap());
        assert_eq!(dur, summary.duration_us);

        // stss：样本 1 与 4
        let stss_r = find_in(&stbl_boxes, "stss");
        let stss = &file[stss_r.0..stss_r.1];
        assert_eq!(u32::from_be_bytes(stss[4..8].try_into().unwrap()), 2);
        assert_eq!(u32::from_be_bytes(stss[8..12].try_into().unwrap()), 1);
        assert_eq!(u32::from_be_bytes(stss[12..16].try_into().unwrap()), 4);

        // stco：单一 chunk，offset = mdat 数据起点
        let stco_r = find_in(&stbl_boxes, "stco");
        let stco = &file[stco_r.0..stco_r.1];
        let chunk_offset = u32::from_be_bytes(stco[8..12].try_into().unwrap());
        assert_eq!(chunk_offset as usize, mdat_s, "stco 指向 mdat 数据起点");

        // avcC：SPS/PPS 原样（必须是 avc1 内的合法子 box）。avc1 载荷开头是
        // VisualSampleEntry 固定字段（非 box），所以直接扫描 avcC fourcc。
        let stsd_r = find_in(&stbl_boxes, "stsd");
        let avc1_start = stsd_r.0 + 8;
        let avc1_size =
            u32::from_be_bytes(file[avc1_start..avc1_start + 4].try_into().unwrap()) as usize;
        assert_eq!(&file[avc1_start + 4..avc1_start + 8], b"avc1");
        let avc1_end = avc1_start + avc1_size;
        let avc1_payload = &file[avc1_start + 8..avc1_end];
        let pos = avc1_payload
            .windows(4)
            .position(|w| w == b"avcC")
            .expect("avcC box 必须在 avc1 内");
        let avcc_start = avc1_start + 8 + pos - 4;
        let avcc_size =
            u32::from_be_bytes(file[avcc_start..avcc_start + 4].try_into().unwrap()) as usize;
        assert_eq!(
            avcc_start + avcc_size,
            avc1_end,
            "avcC 是 avc1 的最后一个子 box 且尺寸自洽"
        );
        let avcc = &file[avcc_start + 8..avcc_start + avcc_size];
        let needle = [0xE1u8, 0, sps.len() as u8];
        let pos = avcc.windows(needle.len()).position(|w| w == needle).expect("avcC SPS 长度前缀");
        assert_eq!(&avcc[pos + 3..pos + 3 + sps.len()], &sps[..]);

        // 样本无损（AVCC 化 = 去参数集 + 4 字节长度前缀）：
        // 首样本 = IDR NAL [5,0xAA] + SEI NAL [6,0x01]
        let idr = [5u8, 0xAA];
        let sei = nal(6, &[0x01]);
        let mut expect = Vec::new();
        expect.extend((idr.len() as u32).to_be_bytes());
        expect.extend(idr);
        expect.extend((sei.len() as u32).to_be_bytes());
        expect.extend(&sei);
        assert_eq!(&mdat[..expect.len()], &expect[..], "首样本 = IDR + SEI（参数集剥离）");

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn mux_rejects_finish_without_config_or_samples() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-mp4b-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.mp4");
        let w = H264Mp4Writer::create(&path, 64, 64).unwrap();
        assert!(w.finish().is_err(), "无样本不允许收口");

        let mut w = H264Mp4Writer::create(&path, 64, 64).unwrap();
        w.write_annexb_sample(&annexb(&[&nal(5, &[1])]), 0, true).unwrap();
        assert!(w.finish().is_err(), "无参数集不允许收口");
        std::fs::remove_dir_all(dir).ok();
    }

    /// 真实解码链冒烟：ffmpeg 生成 H.264 → muxer 封装 → ffprobe 校验帧数/时长。
    /// 依赖外部 ffmpeg/ffprobe，CI 机器不保证存在 → `#[ignore]`（本机验证通过）。
    #[test]
    #[ignore = "需要外部 ffmpeg/ffprobe（本机已验证通过）"]
    fn ignored_playback_smoke_with_ffprobe() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-mp4ff-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("src.h264");
        let out = dir.join("out.mp4");
        let gen = std::process::Command::new("ffmpeg")
            .args([
                "-y", "-loglevel", "error", "-f", "lavfi",
                "-i", "testsrc=duration=2:size=128x96:rate=10",
                "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
                "-x264-params", "keyint=10", "-f", "h264", raw.to_str().unwrap(),
            ])
            .status()
            .expect("ffmpeg 可用");
        assert!(gen.success());
        let raw_bytes = std::fs::read(&raw).unwrap();

        // 逐访问单元喂入：按 slice NAL（5/1）收口一个 AU；参数集经内联剥离
        // 进入 avcC（与 scrcpy repeat-previous-headers 流形一致）
        let mut w = H264Mp4Writer::create(&out, 128, 96).unwrap();
        let mut pts = 0u64;
        let mut au: Vec<Vec<u8>> = Vec::new();
        let mut pending_cfg: Vec<u8> = Vec::new();
        let nals = split_annexb(&raw_bytes);
        let mut saw_config = false;
        for nal_data in &nals {
            let t = nal_type(nal_data);
            if t == 7 || t == 8 {
                saw_config = true;
                pending_cfg.extend_from_slice(&[0, 0, 0, 1]);
                pending_cfg.extend_from_slice(nal_data);
                continue;
            }
            au.push(nal_data.to_vec());
            let closes_au = t == 5 || t == 1;
            if !closes_au {
                continue;
            }
            let mut annexb = std::mem::take(&mut pending_cfg);
            for n in &au {
                annexb.extend([0, 0, 0, 1]);
                annexb.extend(n);
            }
            w.write_annexb_sample(&annexb, pts, t == 5).unwrap();
            pts += 100_000; // 10fps
            au.clear();
        }
        assert!(saw_config, "样本必须含参数集");
        let summary = w.finish().unwrap();
        assert_eq!(summary.samples, 20, "2s@10fps = 20 帧");

        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v", "error", "-select_streams", "v:0", "-show_streams",
                "-show_entries", "stream=nb_frames,duration,codec_name,width,height",
                "-of", "json", out.to_str().unwrap(),
            ])
            .output()
            .expect("ffprobe 可用");
        let text = String::from_utf8_lossy(&probe.stdout).to_string();
        assert!(text.contains("\"h264\""), "{text}");
        assert!(text.contains("128"), "{text}");
        assert!(text.contains("2.0"), "时长应为 2.0s: {text}");

        std::fs::remove_dir_all(dir).ok();
    }
}
