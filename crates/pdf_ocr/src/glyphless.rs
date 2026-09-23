//! OCR 텍스트 레이어용 glyphless 폰트(설계 문서 6.4) — 코드로 직접 만든다(라이선스 문제 없음).
//!
//! - 폰트 프로그램: 글리프 두 개(빈 .notdef, 글자 칸 전체를 덮는 사각형 하나짜리 1번)만 가진 최소
//!   TrueType. 모든 글리프 폭 500, unitsPerEm 1000, ascent 1000, descent 0. 1번 글리프를 비워 두면
//!   pdfium이 글자 하나짜리 텍스트 객체를 크기 0으로 보고 추출에서 빼 버린다(줄에 혼자 있는 한 글자
//!   단어가 사라짐, 왕복 시험 2026-09-22) — Tesseract의 glyphless 폰트도 같은 사각형 윤곽을 쓴다.
//!   렌더 모드 3으로만 쓰므로 화면에는 그려지지 않는다. descent가 0이라 pdfium의 loose box가 "기준선 ~
//!   기준선+글자 크기"가 되어, 기준선을 hOCR 단어 bbox 아래 변에 두고 글자 크기를 bbox 높이로 하면
//!   선택·검색 하이라이트가 bbox와 정확히 겹친다.
//! - PDF 쪽: `Type0` + `Identity-H` + `CIDFontType2`. 문서에 쓰인 문자마다 CID를 1부터 차례로
//!   부여하고(유니코드 값을 CID로 쓰지 않으므로 BMP 밖 문자도 된다), `CIDToGIDMap`은 모두 1번
//!   글리프로 보낸다. 복사·검색 품질은 `ToUnicode`(CID → UTF-16, 서로게이트 쌍 포함)가 정한다.
//! - PDF/A 대응: 폰트 임베딩, `ToUnicode`, `CIDSystemInfo`를 모두 갖춘다. 폭은 `/DW 500`으로
//!   폰트 프로그램의 폭과 같다.
//! - `/BaseFont /PDFOutliner-GlyphLess`로 이름을 고정해, 앱이 넣은 레이어를 알아보는 보조 수단으로 쓴다.

use lopdf::{dictionary, Document, Object, ObjectId, Stream, StringFormat};
use std::collections::HashMap;

pub const FONT_NAME: &str = "PDFOutliner-GlyphLess";
/// 모든 글리프의 폭(1/1000 em).
pub const GLYPH_WIDTH: f64 = 500.0;

/// 문자 → CID 부여(1부터, 처음 나온 순서).
#[derive(Debug, Default, Clone)]
pub struct CidAssigner {
    cids: HashMap<char, u16>,
    order: Vec<char>,
}

impl CidAssigner {
    /// 문자열을 CID 바이트열(2바이트 빅엔디언)로. CID가 바닥나면(65,535자 초과) 그 문자는 뺀다.
    pub fn encode(&mut self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len() * 2);
        for c in text.chars() {
            if let Some(cid) = self.cid(c) {
                out.extend_from_slice(&cid.to_be_bytes());
            }
        }
        out
    }

    fn cid(&mut self, c: char) -> Option<u16> {
        if let Some(&cid) = self.cids.get(&c) {
            return Some(cid);
        }
        let next = u16::try_from(self.order.len() + 1).ok().filter(|&n| n < u16::MAX)?;
        self.cids.insert(c, next);
        self.order.push(c);
        Some(next)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    fn to_unicode_cmap(&self) -> Vec<u8> {
        let mut s = String::from(concat!(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n",
            "/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n",
            "/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n",
            "1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
        ));
        for chunk in self.order.chunks(100).enumerate() {
            let (block, chars) = chunk;
            s.push_str(&format!("{} beginbfchar\n", chars.len()));
            for (i, c) in chars.iter().enumerate() {
                let cid = block * 100 + i + 1;
                let mut units = [0u16; 2];
                let hex: String = c.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect();
                s.push_str(&format!("<{cid:04X}> <{hex}>\n"));
            }
            s.push_str("endbfchar\n");
        }
        s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        s.into_bytes()
    }

    fn cid_to_gid_map(&self) -> Vec<u8> {
        let mut map = vec![0u8, 0u8]; // CID 0 → .notdef
        for _ in 0..self.order.len() {
            map.extend_from_slice(&1u16.to_be_bytes());
        }
        map
    }
}

/// 폰트 객체들을 문서에 넣고 Type0 폰트 id를 돌려준다. 모든 텍스트를 `encode`한 뒤에 부를 것.
pub fn add_font(doc: &mut Document, cids: &CidAssigner) -> ObjectId {
    let mut program = Stream::new(dictionary! { "Length1" => font_program().len() as i64 }, font_program());
    let _ = program.compress();
    let program = doc.add_object(program);
    let descriptor = doc.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => FONT_NAME,
        "Flags" => 4,
        "FontBBox" => vec![0.into(), 0.into(), 500.into(), 1000.into()],
        "ItalicAngle" => 0,
        "Ascent" => 1000,
        "Descent" => 0,
        "CapHeight" => 1000,
        "StemV" => 80,
        "FontFile2" => program,
    });
    let mut gid_map = Stream::new(dictionary! {}, cids.cid_to_gid_map());
    let _ = gid_map.compress();
    let gid_map = doc.add_object(gid_map);
    let cid_font = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => FONT_NAME,
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::String(b"Adobe".to_vec(), StringFormat::Literal),
            "Ordering" => Object::String(b"Identity".to_vec(), StringFormat::Literal),
            "Supplement" => 0,
        },
        "FontDescriptor" => descriptor,
        "DW" => GLYPH_WIDTH as i64,
        "CIDToGIDMap" => gid_map,
    });
    let mut to_unicode = Stream::new(dictionary! {}, cids.to_unicode_cmap());
    let _ = to_unicode.compress();
    let to_unicode = doc.add_object(to_unicode);
    doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => FONT_NAME,
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![cid_font.into()],
        "ToUnicode" => to_unicode,
    })
}

// ------------------------------------------------------------------ TrueType 프로그램

/// 폰트 생성·수정 시각(1904-01-01부터 초) — 2026-01-01 00:00 UTC로 고정해 결과가 늘 같게 한다.
const FONT_DATE: u64 = 2_082_844_800 + 1_767_225_600;

struct Be(Vec<u8>);

impl Be {
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn i16(&mut self, v: i16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
}

fn table_checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// 최소 TrueType 폰트 바이트.
pub fn font_program() -> Vec<u8> {
    let mut head = Be(Vec::new());
    head.u32(0x0001_0000).u32(0x0001_0000).u32(0) // version, revision, checkSumAdjustment(나중에)
        .u32(0x5F0F_3CF5).u16(0x000B).u16(1000) // magic, flags, unitsPerEm
        .bytes(&FONT_DATE.to_be_bytes()).bytes(&FONT_DATE.to_be_bytes()) // created, modified
        .i16(0).i16(0).i16(500).i16(1000) // bbox
        .u16(0).u16(3).i16(2).i16(0).i16(0); // macStyle, lowestRecPPEM, direction, indexToLocFormat, glyphDataFormat

    let mut hhea = Be(Vec::new());
    hhea.u32(0x0001_0000).i16(1000).i16(0).i16(0).u16(500).i16(0).i16(0).i16(500)
        .i16(1).i16(0).i16(0).bytes(&[0; 8]).i16(0).u16(2);

    let mut maxp = Be(Vec::new());
    maxp.u32(0x0001_0000).u16(2).u16(4).u16(1).u16(0).u16(0).u16(2).bytes(&[0; 16]); // 점 4, 윤곽 1

    let mut hmtx = Be(Vec::new());
    hmtx.u16(500).i16(0).u16(500).i16(0);

    // 1번 글리프: (0,0)-(500,1000) 사각형 윤곽 하나(시계 방향, 모두 곡선 위 점).
    let mut glyph = Be(Vec::new());
    glyph.i16(1).i16(0).i16(0).i16(500).i16(1000) // 윤곽 수, bbox
        .u16(3).u16(0) // 마지막 점 번호, 명령 길이
        .bytes(&[0x01; 4]) // 플래그: 곡선 위 점, x·y는 2바이트 증분
        .i16(0).i16(0).i16(500).i16(0) // x 증분
        .i16(0).i16(1000).i16(0).i16(-1000); // y 증분
    let mut glyf = glyph.0;
    while !glyf.len().is_multiple_of(4) {
        glyf.push(0);
    }
    let mut loca = Be(Vec::new());
    loca.u16(0).u16(0).u16((glyf.len() / 2) as u16); // 짧은 형식: 오프셋 ÷ 2

    // cmap: (3,1) format 4, 매핑 없음(끝 구간만). CIDToGIDMap으로 글리프를 고르므로 쓰이지 않지만
    // 일부 구현이 cmap 없는 TrueType을 거부한다.
    let mut cmap = Be(Vec::new());
    cmap.u16(0).u16(1).u16(3).u16(1).u32(12)
        .u16(4).u16(24).u16(0).u16(2).u16(2).u16(0).u16(0)
        .u16(0xFFFF).u16(0).u16(0xFFFF).i16(1).u16(0);

    let names = [(1u16, FONT_NAME), (2, "Regular"), (4, FONT_NAME), (6, FONT_NAME)];
    let encoded: Vec<Vec<u8>> = names.iter().map(|(_, s)| s.encode_utf16().flat_map(u16::to_be_bytes).collect()).collect();
    let mut name = Be(Vec::new());
    name.u16(0).u16(names.len() as u16).u16(6 + 12 * names.len() as u16);
    let mut offset = 0u16;
    for ((id, _), bytes) in names.iter().zip(&encoded) {
        name.u16(3).u16(1).u16(0x409).u16(*id).u16(bytes.len() as u16).u16(offset);
        offset += bytes.len() as u16;
    }
    for bytes in &encoded {
        name.bytes(bytes);
    }

    let mut os2 = Be(Vec::new());
    os2.u16(3).i16(500).u16(400).u16(5).u16(0)
        .bytes(&[0; 20]) // 첨자·취소선 값
        .i16(0).bytes(&[0; 10]).bytes(&[0; 16])
        .bytes(b"PDFO").u16(0x0040).u16(0x0020).u16(0xFFFF)
        .i16(1000).i16(0).i16(0).u16(1000).u16(0)
        .u32(1).u32(0) // 코드 페이지: Latin 1(비어 있으면 거부하는 구현이 있음)
        .i16(500).i16(1000).u16(0).u16(32).u16(0);

    let mut post = Be(Vec::new());
    post.u32(0x0003_0000).u32(0).i16(-100).i16(50).u32(1).bytes(&[0; 16]);

    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"OS/2", os2.0),
        (b"cmap", cmap.0),
        (b"glyf", glyf),
        (b"head", head.0),
        (b"hhea", hhea.0),
        (b"hmtx", hmtx.0),
        (b"loca", loca.0),
        (b"maxp", maxp.0),
        (b"name", name.0),
        (b"post", post.0),
    ];
    tables.sort_by_key(|(tag, _)| **tag);

    let num = tables.len() as u16;
    let entry_selector = 15 - num.leading_zeros() as u16; // floor(log2(num))
    let search_range = 16u16 << entry_selector;
    let mut out = Be(Vec::new());
    out.u32(0x0001_0000).u16(num).u16(search_range).u16(entry_selector).u16(num * 16 - search_range);
    let mut offset = 12 + 16 * tables.len() as u32;
    let mut body = Vec::new();
    let mut head_offset = 0usize;
    for (tag, data) in &tables {
        out.bytes(*tag).u32(table_checksum(data)).u32(offset).u32(data.len() as u32);
        if *tag == b"head" {
            head_offset = offset as usize;
        }
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() as u32 + body.len() as u32;
    }
    let mut font = out.0;
    font.extend_from_slice(&body);
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(table_checksum(&font));
    font[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    font
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cids_are_sequential_and_shared() {
        let mut cids = CidAssigner::default();
        assert_eq!(cids.encode("가나가"), vec![0, 1, 0, 2, 0, 1]);
        assert_eq!(cids.encode("𠀀"), vec![0, 3]); // BMP 밖(U+20000)도 CID 하나
        let cmap = String::from_utf8(cids.to_unicode_cmap()).unwrap();
        assert!(cmap.contains("<0001> <AC00>"));
        assert!(cmap.contains("<0003> <D840DC00>"), "서로게이트 쌍");
        assert_eq!(cids.cid_to_gid_map(), vec![0, 0, 0, 1, 0, 1, 0, 1]);
    }

    #[test]
    fn to_unicode_blocks_have_at_most_100_entries() {
        let mut cids = CidAssigner::default();
        let text: String = (0..250).map(|i| char::from_u32(0xAC00 + i).unwrap()).collect();
        cids.encode(&text);
        let cmap = String::from_utf8(cids.to_unicode_cmap()).unwrap();
        assert_eq!(cmap.matches("100 beginbfchar").count(), 2);
        assert_eq!(cmap.matches("50 beginbfchar").count(), 1);
    }

    #[test]
    fn font_checksum_is_consistent() {
        let font = font_program();
        assert_eq!(font.len() % 4, 0);
        assert_eq!(table_checksum(&font), 0xB1B0_AFBA);
    }
}
