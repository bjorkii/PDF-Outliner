//! 바이트 범위를 보존하는 콘텐츠 스트림 토크나이저(설계 문서 2.2).
//!
//! 연산자마다 원본 바이트에서의 범위(`Operation::span`)를 함께 돌려준다. 편집은 이 범위만
//! 잘라 바꾸고 나머지 바이트는 그대로 둔다 — 공백·주석·모르는 연산자까지 원본 그대로 남는다.
//!
//! 인라인 이미지(`BI … ID <바이너리> EI`)는 바이너리 안에 `EI` 바이트가 나올 수 있어 끝을 찾는
//! 순서를 정해 둔다: (1) `/L`(`/Length`) 값, (2) 필터가 없으면 폭·높이·색 성분으로 계산한 길이,
//! (3) 공백으로 둘러싸인 `EI`를 찾되 그 뒤 토큰들이 정상 연산자로 읽히는지 확인. 세 방법이 모두
//! 실패하면 오류를 돌려주고, 호출 측은 그 페이지를 건드리지 않는다.

use std::ops::Range;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    /// `#xx` 이스케이프를 푼 이름(앞의 `/` 제외).
    Name(Vec<u8>),
    /// 리터럴·16진 문자열 모두 이스케이프를 푼 바이트.
    Str(Vec<u8>),
    Array(Vec<Operand>),
    Dict(Vec<(Vec<u8>, Operand)>),
}

impl Operand {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Operand::Int(i) => Some(*i as f64),
            Operand::Real(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Operand::Name(name) => Some(name),
            _ => None,
        }
    }

    /// 딕셔너리 피연산자에서 키 조회(`BDC`의 속성, 인라인 이미지 딕셔너리).
    pub fn dict_get(&self, key: &[u8]) -> Option<&Operand> {
        match self {
            Operand::Dict(entries) => dict_lookup(entries, key),
            _ => None,
        }
    }
}

pub fn dict_lookup<'a>(entries: &'a [(Vec<u8>, Operand)], key: &[u8]) -> Option<&'a Operand> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

#[derive(Debug, Clone, PartialEq)]
pub struct InlineImage {
    pub dict: Vec<(Vec<u8>, Operand)>,
    /// `ID` 뒤 공백 한 바이트 다음부터 `EI` 앞까지(끝 공백 포함 가능).
    pub data: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    pub operator: Vec<u8>,
    pub operands: Vec<Operand>,
    /// 첫 피연산자(없으면 연산자) 시작부터 연산자 끝까지. 인라인 이미지는 `BI`부터 `EI` 끝까지.
    pub span: Range<usize>,
    pub inline_image: Option<InlineImage>,
}

impl Operation {
    pub fn is(&self, operator: &[u8]) -> bool {
        self.operator == operator
    }

    pub fn operand_f64(&self, index: usize) -> Option<f64> {
        self.operands.get(index).and_then(Operand::as_f64)
    }

    pub fn operand_name(&self, index: usize) -> Option<&[u8]> {
        self.operands.get(index).and_then(Operand::as_name)
    }
}

#[derive(Debug, Clone, Error, PartialEq)]
pub enum LexError {
    #[error("{offset}바이트: 닫히지 않은 문자열")]
    UnterminatedString { offset: usize },
    #[error("{offset}바이트: 닫히지 않은 배열·딕셔너리")]
    UnterminatedContainer { offset: usize },
    #[error("{offset}바이트: 예상하지 못한 구분자 '{byte}'")]
    UnexpectedDelimiter { offset: usize, byte: char },
    #[error("{offset}바이트: 딕셔너리 키가 이름이 아님")]
    BadDictKey { offset: usize },
    #[error("{offset}바이트: 인라인 이미지의 끝(EI)을 찾지 못함")]
    InlineImageEnd { offset: usize },
    #[error("{offset}바이트: 인라인 이미지 딕셔너리 해석 실패")]
    InlineImageDict { offset: usize },
}

/// 콘텐츠 스트림 전체를 연산자 목록으로 나눈다. 연산자 없이 끝에 남은 피연산자는 버린다
/// (pdfium과 같은 동작 — 바이트는 편집 시 그대로 남는다).
pub fn tokenize(data: &[u8]) -> Result<Vec<Operation>, LexError> {
    let mut lexer = Lexer { data, pos: 0 };
    let mut operations = Vec::new();
    let mut operands = Vec::new();
    let mut operands_start: Option<usize> = None;

    loop {
        lexer.skip_whitespace_and_comments();
        let start = lexer.pos;
        let Some(token) = lexer.next_token()? else {
            break;
        };
        match token {
            Token::Operand(operand) => {
                operands_start.get_or_insert(start);
                operands.push(operand);
            }
            Token::Operator(operator) => {
                let span_start = operands_start.take().unwrap_or(start);
                if operator == b"BI" {
                    // BI 앞에 피연산자가 있으면 비정상이지만 범위만 넓혀 함께 둔다.
                    let inline_image = lexer.inline_image(start)?;
                    operations.push(Operation {
                        operator,
                        operands: std::mem::take(&mut operands),
                        span: span_start..lexer.pos,
                        inline_image: Some(inline_image),
                    });
                } else {
                    operations.push(Operation {
                        operator,
                        operands: std::mem::take(&mut operands),
                        span: span_start..lexer.pos,
                        inline_image: None,
                    });
                }
            }
        }
    }
    Ok(operations)
}

/// 알려진 콘텐츠 스트림 연산자(ISO 32000-1 부록 A). 인라인 이미지 끝 판정의 뒤 토큰 검사에만
/// 쓴다 — 본 토크나이저는 모르는 연산자도 그대로 받아들인다(`BX`/`EX` 호환 구역).
const KNOWN_OPERATORS: &[&[u8]] = &[
    b"b", b"B", b"b*", b"B*", b"BDC", b"BI", b"BMC", b"BT", b"BX", b"c", b"cm", b"CS", b"cs", b"d",
    b"d0", b"d1", b"Do", b"DP", b"EI", b"EMC", b"ET", b"EX", b"f", b"F", b"f*", b"G", b"g", b"gs",
    b"h", b"i", b"ID", b"j", b"J", b"K", b"k", b"l", b"m", b"M", b"MP", b"n", b"q", b"Q", b"re",
    b"RG", b"rg", b"ri", b"s", b"S", b"SC", b"sc", b"SCN", b"scn", b"sh", b"T*", b"Tc", b"Td",
    b"TD", b"Tf", b"Tj", b"TJ", b"TL", b"Tm", b"Tr", b"Ts", b"Tw", b"Tz", b"v", b"w", b"W", b"W*",
    b"y", b"'", b"\"",
];

enum Token {
    Operand(Operand),
    Operator(Vec<u8>),
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

fn is_delimiter(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn is_regular(b: u8) -> bool {
    !is_whitespace(b) && !is_delimiter(b)
}

struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn skip_whitespace_and_comments(&mut self) {
        while let Some(b) = self.peek() {
            if is_whitespace(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(b) = self.peek() {
                    if b == b'\r' || b == b'\n' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Result<Option<Token>, LexError> {
        let Some(b) = self.peek() else {
            return Ok(None);
        };
        let start = self.pos;
        let token = match b {
            b'/' => Token::Operand(Operand::Name(self.name())),
            b'(' => Token::Operand(Operand::Str(self.literal_string()?)),
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => Token::Operand(self.dict()?),
            b'<' => Token::Operand(Operand::Str(self.hex_string()?)),
            b'[' => Token::Operand(self.array()?),
            b')' | b'>' | b']' | b'{' | b'}' => {
                return Err(LexError::UnexpectedDelimiter { offset: start, byte: b as char })
            }
            _ => {
                let word = self.word();
                match word {
                    b"true" => Token::Operand(Operand::Bool(true)),
                    b"false" => Token::Operand(Operand::Bool(false)),
                    b"null" => Token::Operand(Operand::Null),
                    _ if matches!(word[0], b'0'..=b'9' | b'+' | b'-' | b'.') => {
                        Token::Operand(parse_number(word))
                    }
                    _ => Token::Operator(word.to_vec()),
                }
            }
        };
        Ok(Some(token))
    }

    fn word(&mut self) -> &'a [u8] {
        let start = self.pos;
        while self.peek().is_some_and(is_regular) {
            self.pos += 1;
        }
        &self.data[start..self.pos]
    }

    fn name(&mut self) -> Vec<u8> {
        self.pos += 1; // '/'
        let raw = self.word();
        let mut out = Vec::with_capacity(raw.len());
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == b'#' && i + 2 < raw.len() {
                if let (Some(h), Some(l)) = (hex_value(raw[i + 1]), hex_value(raw[i + 2])) {
                    out.push(h << 4 | l);
                    i += 3;
                    continue;
                }
            }
            out.push(raw[i]);
            i += 1;
        }
        out
    }

    fn literal_string(&mut self) -> Result<Vec<u8>, LexError> {
        let start = self.pos;
        self.pos += 1; // '('
        let mut depth = 1usize;
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'\\' => {
                    let Some(e) = self.peek() else { break };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut value = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        value = value * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(value as u8);
                        }
                        other => out.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                    out.push(b);
                }
                _ => out.push(b),
            }
        }
        Err(LexError::UnterminatedString { offset: start })
    }

    fn hex_string(&mut self) -> Result<Vec<u8>, LexError> {
        let start = self.pos;
        self.pos += 1; // '<'
        let mut out = Vec::new();
        let mut high: Option<u8> = None;
        while let Some(b) = self.peek() {
            self.pos += 1;
            if b == b'>' {
                if let Some(h) = high {
                    out.push(h << 4);
                }
                return Ok(out);
            }
            // 공백과 16진수가 아닌 바이트는 건너뛴다(pdfium과 같은 관대한 처리).
            if let Some(v) = hex_value(b) {
                match high.take() {
                    Some(h) => out.push(h << 4 | v),
                    None => high = Some(v),
                }
            }
        }
        Err(LexError::UnterminatedString { offset: start })
    }

    /// 배열·딕셔너리 안의 값 하나. 연산자 자리의 단어가 나오면(`R` 참조 등) 이름처럼 보존할
    /// 필요가 없어 무시하지만, 콘텐츠 스트림에는 간접 참조가 없으므로 사실상 나오지 않는다.
    fn value(&mut self) -> Result<Option<Operand>, LexError> {
        match self.next_token()? {
            Some(Token::Operand(operand)) => Ok(Some(operand)),
            Some(Token::Operator(_)) => Ok(None),
            None => Ok(None),
        }
    }

    fn array(&mut self) -> Result<Operand, LexError> {
        let start = self.pos;
        self.pos += 1; // '['
        let mut items = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            match self.peek() {
                None => return Err(LexError::UnterminatedContainer { offset: start }),
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Operand::Array(items));
                }
                Some(_) => {
                    if let Some(item) = self.value()? {
                        items.push(item);
                    }
                }
            }
        }
    }

    fn dict(&mut self) -> Result<Operand, LexError> {
        let start = self.pos;
        self.pos += 2; // '<<'
        let mut entries = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            match self.peek() {
                None => return Err(LexError::UnterminatedContainer { offset: start }),
                Some(b'>') => {
                    if self.data.get(self.pos + 1) == Some(&b'>') {
                        self.pos += 2;
                        return Ok(Operand::Dict(entries));
                    }
                    return Err(LexError::UnexpectedDelimiter { offset: self.pos, byte: '>' });
                }
                Some(b'/') => {
                    let key = self.name();
                    self.skip_whitespace_and_comments();
                    if self.peek().is_none() {
                        return Err(LexError::UnterminatedContainer { offset: start });
                    }
                    let value = self.value()?.unwrap_or(Operand::Null);
                    entries.push((key, value));
                }
                Some(_) => return Err(LexError::BadDictKey { offset: self.pos }),
            }
        }
    }

    /// `BI` 연산자를 읽은 직후 호출 — 딕셔너리, `ID`, 데이터, `EI`까지 소비한다.
    fn inline_image(&mut self, bi_offset: usize) -> Result<InlineImage, LexError> {
        let mut dict = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            match self.peek() {
                None => return Err(LexError::InlineImageDict { offset: bi_offset }),
                Some(b'/') => {
                    let key = self.name();
                    self.skip_whitespace_and_comments();
                    let value = self
                        .value()
                        .map_err(|_| LexError::InlineImageDict { offset: bi_offset })?
                        .ok_or(LexError::InlineImageDict { offset: bi_offset })?;
                    dict.push((key, value));
                }
                Some(_) => {
                    let word = self.word();
                    if word == b"ID" {
                        break;
                    }
                    return Err(LexError::InlineImageDict { offset: bi_offset });
                }
            }
        }
        // ID 뒤 공백 한 바이트(규격). 공백이 없으면 바로 데이터로 본다.
        if self.peek().is_some_and(is_whitespace) {
            self.pos += 1;
        }
        let data_start = self.pos;

        let explicit = dict_lookup(&dict, b"L")
            .or_else(|| dict_lookup(&dict, b"Length"))
            .and_then(Operand::as_f64)
            .map(|v| v as usize);
        let candidates = [explicit, computed_inline_image_length(&dict)];
        for length in candidates.into_iter().flatten() {
            if let Some(end) = self.ei_after(data_start.saturating_add(length)) {
                self.pos = end;
                return Ok(InlineImage { dict, data: data_start..data_start + length });
            }
        }

        // 공백 + EI + (공백·구분자·끝), 그리고 뒤 토큰이 정상 연산자로 읽히는 첫 위치.
        let mut i = data_start;
        while i + 2 <= self.data.len() {
            if &self.data[i..i + 2] == b"EI"
                && (i == data_start || is_whitespace(self.data[i - 1]))
                && self.data.get(i + 2).is_none_or(|&b| is_whitespace(b) || is_delimiter(b))
                && following_tokens_look_valid(self.data, i + 2)
            {
                self.pos = i + 2;
                return Ok(InlineImage { dict, data: data_start..i });
            }
            i += 1;
        }
        Err(LexError::InlineImageEnd { offset: bi_offset })
    }

    /// `at`부터 공백을 건너뛰고 `EI`가 온전한 토큰으로 있으면 그 끝 위치.
    fn ei_after(&self, at: usize) -> Option<usize> {
        let mut i = at;
        while self.data.get(i).is_some_and(|&b| is_whitespace(b)) {
            i += 1;
        }
        (self.data.get(i..i + 2) == Some(b"EI")
            && self.data.get(i + 2).is_none_or(|&b| is_whitespace(b) || is_delimiter(b)))
        .then_some(i + 2)
    }
}

/// 필터가 없고 색 성분을 알 수 있을 때의 데이터 길이(행마다 바이트 경계로 맞춤).
fn computed_inline_image_length(dict: &[(Vec<u8>, Operand)]) -> Option<usize> {
    let get = |short: &[u8], long: &[u8]| dict_lookup(dict, short).or_else(|| dict_lookup(dict, long));
    match get(b"F", b"Filter") {
        None => {}
        Some(Operand::Array(filters)) if filters.is_empty() => {}
        Some(_) => return None,
    }
    let width = get(b"W", b"Width")?.as_f64()? as usize;
    let height = get(b"H", b"Height")?.as_f64()? as usize;
    let is_mask = matches!(get(b"IM", b"ImageMask"), Some(Operand::Bool(true)));
    let (components, bpc) = if is_mask {
        (1, 1)
    } else {
        let components = match get(b"CS", b"ColorSpace")? {
            Operand::Name(name) => match name.as_slice() {
                b"G" | b"DeviceGray" | b"CalGray" => 1,
                b"RGB" | b"DeviceRGB" | b"CalRGB" => 3,
                b"CMYK" | b"DeviceCMYK" => 4,
                _ => return None, // 리소스 이름 — 성분 수를 여기서 알 수 없음
            },
            Operand::Array(items) if items.first().and_then(Operand::as_name).is_some_and(|n| n == b"I" || n == b"Indexed") => 1,
            _ => return None,
        };
        (components, get(b"BPC", b"BitsPerComponent")?.as_f64()? as usize)
    };
    let row = (width.checked_mul(components)?.checked_mul(bpc)?).div_ceil(8);
    row.checked_mul(height)
}

/// `EI` 뒤가 정상 콘텐츠로 이어지는지 — 다음 몇 개 토큰이 문법 오류 없이 읽히고 연산자는 모두
/// 알려진 것이어야 한다. 스트림 끝이면 참.
fn following_tokens_look_valid(data: &[u8], from: usize) -> bool {
    let mut lexer = Lexer { data, pos: from };
    for _ in 0..8 {
        lexer.skip_whitespace_and_comments();
        match lexer.next_token() {
            Ok(None) => return true,
            Ok(Some(Token::Operand(_))) => {}
            Ok(Some(Token::Operator(op))) => {
                if !KNOWN_OPERATORS.contains(&op.as_slice()) {
                    return false;
                }
                if op == b"BI" {
                    return true;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 관대한 숫자 해석 — `4.`, `-.5`, `+3`을 받고, `1.2.3`처럼 망가진 토큰은 읽을 수 있는 앞부분만
/// 쓴다(pdfium도 비슷하게 동작).
fn parse_number(word: &[u8]) -> Operand {
    let text = std::str::from_utf8(word).unwrap_or("0");
    if !text.contains('.') {
        if let Ok(i) = text.parse::<i64>() {
            return Operand::Int(i);
        }
    }
    let mut end = 0;
    let mut seen_dot = false;
    for (i, c) in text.char_indices() {
        match c {
            '+' | '-' if i == 0 => {}
            '.' if !seen_dot => seen_dot = true,
            '0'..='9' => {}
            _ => break,
        }
        end = i + 1;
    }
    let prefix = &text[..end];
    let value = match prefix {
        "" | "+" | "-" | "." | "+." | "-." => 0.0,
        _ => prefix.trim_end_matches('.').parse::<f64>().or_else(|_| format!("{prefix}0").parse()).unwrap_or(0.0),
    };
    Operand::Real(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(data: &[u8]) -> Vec<Operation> {
        tokenize(data).expect("tokenize 실패")
    }

    #[test]
    fn spans_cover_operands_and_operator() {
        let data = b"q 1 0 0 1 10 20 cm\nBT /F1 12 Tf (Hi) Tj ET Q";
        let ops = ops(data);
        let names: Vec<&[u8]> = ops.iter().map(|o| o.operator.as_slice()).collect();
        assert_eq!(names, vec![&b"q"[..], b"cm", b"BT", b"Tf", b"Tj", b"ET", b"Q"]);
        assert_eq!(&data[ops[1].span.clone()], b"1 0 0 1 10 20 cm");
        assert_eq!(&data[ops[3].span.clone()], b"/F1 12 Tf");
        assert_eq!(&data[ops[4].span.clone()], b"(Hi) Tj");
        assert_eq!(ops[4].operands, vec![Operand::Str(b"Hi".to_vec())]);
    }

    #[test]
    fn strings_escapes_and_nesting() {
        let ops = ops(b"(a\\(b\\)c (nested) \\101\\n) Tj <48 65 6c6C 6f> Tj <4> Tj");
        assert_eq!(ops[0].operands[0], Operand::Str(b"a(b)c (nested) A\n".to_vec()));
        assert_eq!(ops[1].operands[0], Operand::Str(b"Hello".to_vec()));
        assert_eq!(ops[2].operands[0], Operand::Str(vec![0x40]));
    }

    #[test]
    fn tj_array_dict_and_names() {
        let ops = ops(b"[(A) -120 (B)] TJ /Span <</ActualText (x) /MCID 3>> BDC /A#20B gs");
        assert_eq!(
            ops[0].operands[0],
            Operand::Array(vec![Operand::Str(b"A".to_vec()), Operand::Int(-120), Operand::Str(b"B".to_vec())])
        );
        assert_eq!(ops[1].operands[1].dict_get(b"MCID"), Some(&Operand::Int(3)));
        assert_eq!(ops[2].operand_name(0), Some(&b"A B"[..]));
    }

    #[test]
    fn comments_and_lenient_numbers() {
        let ops = ops(b"% comment\n4. -.5 +3 1.2.3 cm");
        assert_eq!(ops[0].operands, vec![Operand::Real(4.0), Operand::Real(-0.5), Operand::Int(3), Operand::Real(1.2)]);
    }

    #[test]
    fn inline_image_with_ei_bytes_inside_data() {
        // 4x1 RGB, 필터 없음 → 계산 길이 12바이트. 데이터 안에 " EI " 바이트가 들어 있다.
        let mut data = b"q BI /W 4 /H 1 /CS /RGB /BPC 8 ID ".to_vec();
        data.extend_from_slice(b"\x01 EI \x02\x03\x04\x05\x06\x07\x08");
        data.extend_from_slice(b" EI Q");
        let ops = ops(&data);
        let names: Vec<&[u8]> = ops.iter().map(|o| o.operator.as_slice()).collect();
        assert_eq!(names, vec![&b"q"[..], b"BI", b"Q"]);
        let image = ops[1].inline_image.as_ref().unwrap();
        assert_eq!(image.data.len(), 12);
    }

    #[test]
    fn inline_image_filtered_uses_lookahead() {
        // 필터가 있어 길이를 모름 — 데이터 안의 "EI"는 뒤가 정상 연산자가 아니라 건너뛴다.
        let mut data = b"BI /W 10 /H 10 /CS /G /BPC 8 /F /AHx ID ".to_vec();
        data.extend_from_slice(b"00 EI zz 11>");
        data.extend_from_slice(b" EI 0 0 m");
        let ops = ops(&data);
        assert_eq!(ops.len(), 2);
        assert_eq!(&data[ops[0].inline_image.as_ref().unwrap().data.clone()], b"00 EI zz 11> ");
        assert!(ops[1].is(b"m"));
    }

    #[test]
    fn inline_image_explicit_length() {
        let mut data = b"BI /W 1 /H 1 /F /DCT /L 4 ID ".to_vec();
        data.extend_from_slice(b"EI\n\x00");
        data.extend_from_slice(b" EI Q");
        let ops = ops(&data);
        assert_eq!(ops[0].inline_image.as_ref().unwrap().data.len(), 4);
        assert!(ops[1].is(b"Q"));
    }

    #[test]
    fn broken_streams_report_errors() {
        assert!(matches!(tokenize(b"(abc Tj"), Err(LexError::UnterminatedString { .. })));
        assert!(matches!(tokenize(b"[1 2 TJ"), Err(LexError::UnterminatedContainer { .. })));
        assert!(matches!(tokenize(b"BI /W 1 ID xxxx"), Err(LexError::InlineImageEnd { .. })));
    }

    #[test]
    fn unknown_operators_are_kept() {
        let ops = ops(b"BX 1 2 foo EX");
        assert!(ops[1].is(b"foo"));
        assert_eq!(ops[1].operands.len(), 2);
    }
}
