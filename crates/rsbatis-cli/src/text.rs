//! 终端文本对齐。
//!
//! `status` 要按列对齐，而迁移名里出现中文是常态（`20260101_创建用户.sql`）。
//! 中文与 emoji 在终端里占两列，用 `chars().count()` 补空格会歪——
//! 这个文件只做一件小事：算显示宽度，再补到目标宽度。

/// 单字符显示宽度。CJK / 全角 / emoji 记 2，其余记 1。
///
/// 不追求 UAX #11 完整表（那需要几百行区间表），只覆盖实际会出现在迁移名里的：
/// 汉字、假名、全角标点、常见 emoji。规则错了的后果只是某一行歪一列，
/// 不影响数据。
fn char_width(c: char) -> usize {
    let cp = c as u32;
    let wide = matches!(cp,
        0x1100..=0x115F        // 谚文字母
        | 0x2E80..=0x303E      // 部首、CJK 标点
        | 0x3041..=0x33FF      // 假名、CJK 兼容
        | 0x3400..=0x4DBF      // CJK 扩展 A
        | 0x4E00..=0x9FFF      // CJK 基本区
        | 0xA000..=0xA4CF      // 彝文
        | 0xAC00..=0xD7A3      // 谚文音节
        | 0xF900..=0xFAFF      // CJK 兼容表意
        | 0xFE10..=0xFE19      // 竖排标点
        | 0xFE30..=0xFE6F      // CJK 兼容形式
        | 0xFF00..=0xFF60      // 全角 ASCII
        | 0xFFE0..=0xFFE6      // 全角符号
        | 0x1F300..=0x1F6FF    // emoji（🚀 U+1F680 落在 1F600 段之后，1F64F 盖不到）
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD    // CJK 扩展 B 及以上
    );
    if wide {
        2
    } else {
        1
    }
}

/// 字符串在终端里占多少列。
pub fn width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// 左对齐补到 `target` 列宽（已超宽就原样返回，不截断——截断一个迁移名
/// 比让表格歪掉更糟）。
pub fn pad(s: &str, target: usize) -> String {
    let w = width(s);
    if w >= target {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(target - w))
    }
}

/// 右对齐（版本号这种定宽数字用）。
pub fn pad_left(s: &str, target: usize) -> String {
    let w = width(s);
    if w >= target {
        s.to_string()
    } else {
        format!("{}{}", " ".repeat(target - w), s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_one_per_char() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width(""), 0);
    }

    #[test]
    fn cjk_counts_double() {
        assert_eq!(width("创建用户"), 8);
        assert_eq!(width("a创b"), 4);
    }

    #[test]
    fn fullwidth_punct_is_double() {
        assert_eq!(width("，。"), 4);
    }

    #[test]
    fn emoji_counts_double() {
        assert_eq!(width("🚀"), 2);
    }

    #[test]
    fn pad_aligns_cjk_and_ascii_to_same_column() {
        assert_eq!(width(&pad("abc", 8)), 8);
        assert_eq!(width(&pad("创建", 8)), 8);
        assert_eq!(width(&pad("创建用户", 8)), 8);
    }

    #[test]
    fn pad_never_truncates() {
        assert_eq!(pad("abcdefghij", 4), "abcdefghij");
        assert_eq!(pad("创建用户超过", 4), "创建用户超过");
    }

    #[test]
    fn pad_left_for_numbers() {
        assert_eq!(pad_left("42", 6), "    42");
    }
}
