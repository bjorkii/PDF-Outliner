//! 표시 연산자의 텍스트 이동량 계산(설계 문서 4.2).
//!
//! 텍스트 표시 연산자를 지우면 그만큼의 텍스트 행렬 이동도 사라진다. 같은 `BT` 안에서 위치를
//! 다시 잡지 않고 다른 표시 연산자가 이어지면 뒤 글자가 밀리므로, 지운 자리에 같은 이동량의
//! `[n] TJ`를 넣어야 한다. 그 이동량을 폰트 폭으로 계산한다.
//!
//! 계산할 수 있는 폰트: 단순 폰트(`/Widths`, `/FirstChar`, `/MissingWidth`), Type3
//! (`/FontMatrix` 반영), Type0 + `Identity-H`(`/W`, `/DW`). 표준 14 폰트처럼 `/Widths`가 없는
//! 단순 폰트, 다른 CMap을 쓰는 Type0, 세로쓰기는 `None` — 호출 측은 그 페이지를 건너뛴다.

use crate::content::lexer::Operand;
use crate::geometry::{number, resolve};
use lopdf::{Dictionary, Document, Object};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum FontMetrics {
    Simple {
        first_char: u32,
        widths: Vec<f64>,
        missing: f64,
        /// 글리프 공간 → 텍스트 공간 배율(일반 폰트 1/1000, Type3는 FontMatrix의 a).
        scale: f64,
    },
    /// Type0 + Identity-H: 2바이트 코드 = CID.
    Cid { widths: HashMap<u32, f64>, default: f64 },
}

impl FontMetrics {
    pub fn load(doc: &Document, font: &Dictionary) -> Option<Self> {
        let subtype = font.get(b"Subtype").ok()?.as_name().ok()?;
        match subtype {
            b"Type0" => {
                let encoding = font.get(b"Encoding").ok()?.as_name().ok()?;
                if encoding != b"Identity-H" {
                    return None;
                }
                let descendant = resolve(doc, font.get(b"DescendantFonts").ok()?).as_array().ok()?.first()?;
                let cid_font = resolve(doc, descendant).as_dict().ok()?;
                let default = cid_font.get(b"DW").ok().and_then(|o| number(resolve(doc, o))).unwrap_or(1000.0);
                let mut widths = HashMap::new();
                if let Ok(w) = cid_font.get(b"W") {
                    parse_cid_widths(doc, resolve(doc, w).as_array().ok()?, &mut widths)?;
                }
                Some(FontMetrics::Cid { widths, default })
            }
            b"Type1" | b"MMType1" | b"TrueType" | b"Type3" => {
                let widths: Vec<f64> = resolve(doc, font.get(b"Widths").ok()?)
                    .as_array()
                    .ok()?
                    .iter()
                    .map(|o| number(resolve(doc, o)).unwrap_or(0.0))
                    .collect();
                let first_char = font.get(b"FirstChar").ok().and_then(|o| number(resolve(doc, o))).unwrap_or(0.0) as u32;
                let missing = font
                    .get(b"FontDescriptor")
                    .ok()
                    .and_then(|o| resolve(doc, o).as_dict().ok())
                    .and_then(|d| d.get(b"MissingWidth").ok())
                    .and_then(|o| number(resolve(doc, o)))
                    .unwrap_or(0.0);
                let scale = if subtype == b"Type3" {
                    let matrix = resolve(doc, font.get(b"FontMatrix").ok()?).as_array().ok()?;
                    number(resolve(doc, matrix.first()?))?
                } else {
                    0.001
                };
                Some(FontMetrics::Simple { first_char, widths, missing, scale })
            }
            _ => None,
        }
    }

    /// 문자열 하나의 가로 이동량(텍스트 공간, `Th` 적용 전). 규격 9.4.4:
    /// `tx = (w0 × Tfs + Tc + Tw(1바이트 코드 32일 때)) × Th` 중 괄호 안.
    fn string_advance(&self, bytes: &[u8], font_size: f64, char_spacing: f64, word_spacing: f64) -> Option<f64> {
        let mut total = 0.0;
        match self {
            FontMetrics::Simple { first_char, widths, missing, scale } => {
                for &code in bytes {
                    let w = (code as u32)
                        .checked_sub(*first_char)
                        .and_then(|i| widths.get(i as usize))
                        .copied()
                        .unwrap_or(*missing);
                    total += w * scale * font_size + char_spacing;
                    if code == 32 {
                        total += word_spacing;
                    }
                }
            }
            FontMetrics::Cid { widths, default } => {
                if !bytes.len().is_multiple_of(2) {
                    return None;
                }
                for pair in bytes.chunks(2) {
                    let cid = u32::from(pair[0]) << 8 | u32::from(pair[1]);
                    let w = widths.get(&cid).copied().unwrap_or(*default);
                    total += w / 1000.0 * font_size + char_spacing;
                }
            }
        }
        Some(total)
    }

    /// 표시 연산자(`Tj`, `TJ`, `'`, `"`)의 피연산자 전체 이동량(텍스트 공간, `Th` 적용 후).
    /// `"`는 연산자가 설정하는 단어·문자 간격을 먼저 반영해야 하므로 호출 측이 넘긴 값을 쓴다.
    pub fn show_advance(
        &self,
        operator: &[u8],
        operands: &[Operand],
        font_size: f64,
        char_spacing: f64,
        word_spacing: f64,
        horizontal_scaling: f64,
    ) -> Option<f64> {
        let th = horizontal_scaling / 100.0;
        let (tc, tw) = match operator {
            b"\"" => (operands.get(1)?.as_f64()?, operands.first()?.as_f64()?),
            _ => (char_spacing, word_spacing),
        };
        let mut total = 0.0;
        match operator {
            b"TJ" => {
                let Operand::Array(items) = operands.first()? else { return None };
                for item in items {
                    match item {
                        Operand::Str(bytes) => total += self.string_advance(bytes, font_size, tc, tw)?,
                        other => total -= other.as_f64()? / 1000.0 * font_size,
                    }
                }
            }
            b"Tj" | b"'" | b"\"" => {
                let Operand::Str(bytes) = operands.last()? else { return None };
                total = self.string_advance(bytes, font_size, tc, tw)?;
            }
            _ => return None,
        }
        Some(total * th)
    }
}

/// `/W` 배열: `c [w1 w2 …]` 또는 `c_first c_last w` 형식의 반복.
fn parse_cid_widths(doc: &Document, array: &[Object], out: &mut HashMap<u32, f64>) -> Option<()> {
    let mut i = 0;
    while i < array.len() {
        let first = number(resolve(doc, &array[i]))? as u32;
        match resolve(doc, array.get(i + 1)?) {
            Object::Array(ws) => {
                for (k, w) in ws.iter().enumerate() {
                    out.insert(first + k as u32, number(resolve(doc, w))?);
                }
                i += 2;
            }
            last => {
                let last = number(last)? as u32;
                let w = number(resolve(doc, array.get(i + 2)?))?;
                if last.saturating_sub(first) > 0x10000 {
                    return None; // 손상된 범위
                }
                for cid in first..=last {
                    out.insert(cid, w);
                }
                i += 3;
            }
        }
    }
    Some(())
}

/// `[n] TJ`로 `advance`(텍스트 공간, Th 적용 후)만큼 옮길 때의 n. 폰트 크기나 가로 배율이 0이면
/// 숫자로 이동을 표현할 수 없다(None).
pub fn tj_number_for_advance(advance: f64, font_size: f64, horizontal_scaling: f64) -> Option<f64> {
    let unit = font_size * horizontal_scaling / 100.0 / 1000.0;
    if advance.abs() < 1e-9 {
        return Some(0.0);
    }
    (unit.abs() > 1e-12).then(|| -advance / unit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn simple() -> FontMetrics {
        FontMetrics::Simple { first_char: 65, widths: vec![500.0, 600.0], missing: 250.0, scale: 0.001 }
    }

    #[test]
    fn simple_font_tj_with_spacing() {
        // "AB " (B=600, ' '=missing 250) at 10pt, Tc 1, Tw 2, Tz 50
        let ops = [Operand::Str(b"AB ".to_vec())];
        let adv = simple().show_advance(b"Tj", &ops, 10.0, 1.0, 2.0, 50.0).unwrap();
        // (5 + 1) + (6 + 1) + (2.5 + 1 + 2) = 18.5, × 0.5
        assert!((adv - 9.25).abs() < 1e-9);
        // [n] TJ 로 같은 이동: n = -9.25 / (10 × 0.5 / 1000)
        let n = tj_number_for_advance(adv, 10.0, 50.0).unwrap();
        let back = simple().show_advance(b"TJ", &[Operand::Array(vec![Operand::Real(n)])], 10.0, 1.0, 2.0, 50.0).unwrap();
        assert!((back - adv).abs() < 1e-9);
    }

    #[test]
    fn tj_array_kerning_and_quote_operator() {
        let ops = [Operand::Array(vec![Operand::Str(b"A".to_vec()), Operand::Int(-500), Operand::Str(b"B".to_vec())])];
        let adv = simple().show_advance(b"TJ", &ops, 10.0, 0.0, 0.0, 100.0).unwrap();
        assert!((adv - (5.0 + 5.0 + 6.0)).abs() < 1e-9);
        // " 는 자기 피연산자의 간격을 쓴다: aw=3 ac=1
        let ops = [Operand::Int(3), Operand::Int(1), Operand::Str(b" ".to_vec())];
        let adv = simple().show_advance(b"\"", &ops, 10.0, 0.0, 0.0, 100.0).unwrap();
        assert!((adv - (2.5 + 1.0 + 3.0)).abs() < 1e-9);
    }

    #[test]
    fn cid_font_identity_h() {
        let mut doc = Document::with_version("1.7");
        let cid = doc.add_object(dictionary! {
            "DW" => 1000,
            "W" => vec![1.into(), vec![500.into(), 700.into()].into(), 10.into(), 12.into(), 300.into()],
        });
        let font = dictionary! { "Subtype" => "Type0", "Encoding" => "Identity-H", "DescendantFonts" => vec![cid.into()] };
        let m = FontMetrics::load(&doc, &font).unwrap();
        let ops = [Operand::Str(vec![0, 1, 0, 2, 0, 11, 0, 99])];
        let adv = m.show_advance(b"Tj", &ops, 1.0, 0.0, 0.0, 100.0).unwrap();
        assert!((adv - (0.5 + 0.7 + 0.3 + 1.0)).abs() < 1e-9);
        let font = dictionary! { "Subtype" => "Type0", "Encoding" => "UniKS-UCS2-H", "DescendantFonts" => vec![cid.into()] };
        assert!(FontMetrics::load(&doc, &font).is_none());
    }

    #[test]
    fn zero_size_cannot_be_expressed() {
        assert_eq!(tj_number_for_advance(0.0, 0.0, 100.0), Some(0.0));
        assert_eq!(tj_number_for_advance(3.0, 0.0, 100.0), None);
    }
}
