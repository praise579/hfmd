/// 小于这个大小的文件不切分，直接单流下载。
pub const SINGLE_STREAM_BELOW: u64 = 8 * 1024 * 1024;

/// 分片的目标大小区间。片数由文件大小决定，再受 `--connections` 封顶。
pub const MIN_SEGMENT: u64 = 16 * 1024 * 1024;
pub const MAX_SEGMENT: u64 = 64 * 1024 * 1024;

/// 文件内一段连续的字节区间，`end` 为闭区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub index: usize,
    pub start: u64,
    pub end: u64,
}

impl Segment {
    /// 分片覆盖的字节数。
    pub fn size(&self) -> u64 {
        self.end - self.start + 1
    }
}

/// 规划一个文件要切成哪些分片。
///
/// - `file_size == 0` 返回空列表（空文件没有可下载的字节）
/// - 小于 8 MiB 的文件只返回一个分片
/// - 否则片数取「让每片不超过 64 MiB」所需的最少片数，再受 `connections` 封顶
/// - 片数不会多到让某片小于 16 MiB（除非 `connections` 本身过大）
pub fn plan(file_size: u64, connections: usize) -> Vec<Segment> {
    if file_size == 0 {
        return Vec::new();
    }

    let connections = connections.max(1) as u64;

    if file_size < SINGLE_STREAM_BELOW {
        return vec![Segment {
            index: 0,
            start: 0,
            end: file_size - 1,
        }];
    }

    // 每片不超过 MAX_SEGMENT 所需的最少片数
    let min_count = file_size.div_ceil(MAX_SEGMENT);
    // 每片不小于 MIN_SEGMENT 允许的最多片数
    let max_count = (file_size / MIN_SEGMENT).max(1);
    let count = min_count.min(connections).min(max_count).max(1);

    let base = file_size / count;
    let remainder = file_size % count;

    let mut segments = Vec::with_capacity(count as usize);
    let mut start = 0;
    for index in 0..count {
        let len = base + u64::from(index < remainder);
        let end = start + len - 1;
        segments.push(Segment {
            index: index as usize,
            start,
            end,
        });
        start = end + 1;
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;

    fn assert_well_formed(segments: &[Segment], file_size: u64) {
        assert_eq!(segments.first().unwrap().start, 0, "应从 0 开始");
        assert_eq!(
            segments.last().unwrap().end,
            file_size - 1,
            "应覆盖到文件末尾"
        );

        for (index, segment) in segments.iter().enumerate() {
            assert_eq!(segment.index, index, "索引应连续");
            assert!(segment.end >= segment.start, "区间不能反向");
            if index > 0 {
                assert_eq!(
                    segment.start,
                    segments[index - 1].end + 1,
                    "分片之间不能有空洞或重叠"
                );
            }
        }

        let total: u64 = segments.iter().map(Segment::size).sum();
        assert_eq!(total, file_size, "分片长度之和应等于文件大小");
    }

    #[test]
    fn 空文件不产生分片() {
        assert!(plan(0, 8).is_empty());
    }

    #[test]
    fn 小文件单流() {
        for size in [1, 1024, SINGLE_STREAM_BELOW - 1] {
            let segments = plan(size, 8);
            assert_eq!(segments.len(), 1, "size={size}");
            assert_eq!(segments[0].start, 0);
            assert_eq!(segments[0].end, size - 1);
        }
    }

    #[test]
    fn 阈值处开始切分() {
        let segments = plan(SINGLE_STREAM_BELOW, 8);
        assert_eq!(
            segments.len(),
            1,
            "8 MiB 恰好仍是一片 8 MiB，低于 16 MiB 下限"
        );

        let segments = plan(20 * MIB, 8);
        assert_eq!(segments.len(), 1, "20 MiB 切成多片会低于 16 MiB 下限");
    }

    #[test]
    fn 中等文件按_16_到_64_mib_切() {
        let segments = plan(100 * MIB, 8);
        assert_eq!(segments.len(), 2);
        assert_well_formed(&segments, 100 * MIB);
        for segment in &segments {
            assert!((MIN_SEGMENT..=MAX_SEGMENT).contains(&segment.size()));
        }
    }

    #[test]
    fn 大文件受_connections_封顶() {
        let segments = plan(10 * GIB, 8);
        assert_eq!(segments.len(), 8);
        assert_well_formed(&segments, 10 * GIB);

        let segments = plan(10 * GIB, 16);
        assert_eq!(segments.len(), 16);
        assert_well_formed(&segments, 10 * GIB);
    }

    #[test]
    fn connections_为零时至少一片() {
        let segments = plan(100 * MIB, 0);
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn 任何尺寸都无空洞无重叠() {
        for size in [
            1,
            7 * MIB,
            8 * MIB,
            9 * MIB,
            63 * MIB,
            65 * MIB,
            200 * MIB,
            GIB,
        ] {
            for connections in [1, 2, 3, 8, 16] {
                let segments = plan(size, connections);
                assert!(segments.len() <= connections.max(1), "size={size}");
                assert_well_formed(&segments, size);
            }
        }
    }

    #[test]
    fn 分片大小差距不超过一字节() {
        let segments = plan(100 * MIB + 7, 8);
        let min = segments.iter().map(Segment::size).min().unwrap();
        let max = segments.iter().map(Segment::size).max().unwrap();
        assert!(max - min <= 1);
    }
}
