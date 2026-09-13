//! 렌더링 전용 보조 프로세스 — 같은 실행 파일을 `--render-worker`로 하나 더 띄워 페이지
//! 렌더링만 맡긴다.
//!
//! 왜 스레드가 아니라 프로세스인가: pdfium은 스레드 안전하지 않아 한 프로세스 안에서 두
//! 스레드가 동시에 부르면 세그폴트가 난다(`pdf_engine::search` 모듈 문서). 프로세스는 메모리가
//! 분리돼 각자 자기 pdfium을 가지므로 충돌할 대상이 없다. 메인 프로세스는 지금처럼 텍스트
//! 선택·링크·검색에 자기 pdfium을 쓰고 렌더링만 여기에 요청한다 — 고배율 렌더링이 수백 ms
//! 걸려도 UI 스레드는 멈추지 않는다.
//!
//! 통신은 보조 프로세스의 stdin(요청)/stdout(응답) 파이프에 직접 정의한 이진 프레임으로 한다.
//! **보조 프로세스 코드 경로에서 stdout에 다른 것을 쓰면 프로토콜이 깨진다**(로그는 stderr로).
//!
//! 오래된 요청 버리기: 요청마다 `epoch`(메인이 보는 화면이 바뀔 때마다 증가)를 붙이고, 보조
//! 프로세스는 대기열에서 최신 epoch보다 오래된 요청은 렌더링하지 않고 `Dropped`로 돌려준다
//! (SumatraPDF `RenderCache::ClearQueueForDisplayModel`과 같은 역할). 이미 시작한 렌더는
//! pdfium이 중간에 끊을 수 없어 끝까지 간다.

use pdf_engine::PdfEngine;
use pdfium_render::prelude::*;
use std::collections::VecDeque;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

/// 이 인자로 실행되면 창을 만들지 않고 보조 프로세스로 동작한다(main.rs).
pub const WORKER_FLAG: &str = "--render-worker";

const TAG_OPEN: u8 = 1;
const TAG_RENDER: u8 = 2;
const TAG_RENDERED: u8 = 1;
const TAG_FAILED: u8 = 2;
const TAG_DROPPED: u8 = 3;
/// 경로·오류 메시지 길이 상한 — 깨진 프레임으로 거대한 할당을 하지 않게.
const MAX_TEXT_BYTES: usize = 1 << 16;
/// 비트맵 픽셀 수 상한(GPU 텍스처 한도 16384² — app::clamped_render_width).
const MAX_PIXELS: u64 = 16384 * 16384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderRequest {
    /// 문서 세대 — 메인이 문서를 열 때마다 증가. 다른 세대의 요청·응답은 버린다.
    pub generation: u64,
    pub epoch: u64,
    /// 1부터 시작하는 페이지 번호.
    pub page: u32,
    /// 메인이 캐시 키로 쓰는 요청 폭(줌 반영, px). 응답에 그대로 돌려준다.
    pub target_width: i32,
    /// 실제 렌더 폭 — target_width를 GPU 텍스처 한도 안으로 줄인 값.
    pub render_width: i32,
}

#[derive(Debug, Clone, PartialEq)]
enum Request {
    Open { generation: u64, path: String },
    Render(RenderRequest),
}

#[derive(Debug, PartialEq)]
enum Response {
    Rendered {
        generation: u64,
        page: u32,
        target_width: i32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Failed {
        generation: u64,
        page: u32,
        target_width: i32,
        message: String,
    },
    Dropped {
        generation: u64,
        page: u32,
        target_width: i32,
    },
}

/// 메인 프로세스가 매 프레임 받아 가는 결과.
pub enum WorkerEvent {
    Rendered {
        generation: u64,
        page: u32,
        target_width: i32,
        /// 텍스처 등록은 메인 스레드가 한다(app::poll_render_worker).
        image: egui::ColorImage,
    },
    /// 렌더링하지 않고 버렸거나(error: None) 실패함 — 메인은 "요청 중" 표시만 지운다.
    Finished {
        generation: u64,
        page: u32,
        target_width: i32,
        error: Option<String>,
    },
    /// 보조 프로세스가 종료됨(파이프 끊김) — 메인은 동기 렌더링으로 대체한다.
    Died,
}

/// 메인 프로세스 쪽 핸들. 드롭되면 보조 프로세스를 종료한다.
pub struct RenderWorker {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    events: Receiver<WorkerEvent>,
}

impl RenderWorker {
    /// 자기 실행 파일을 보조 프로세스로 띄우고, 응답을 읽는 스레드를 시작한다.
    pub fn spawn(ctx: &egui::Context) -> io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg(WORKER_FLAG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| invalid("stdin 파이프 없음"))?;
        let stdout = child.stdout.take().ok_or_else(|| invalid("stdout 파이프 없음"))?;
        let (sender, events) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("render-worker-reader".to_string())
            .spawn(move || read_responses(stdout, sender, ctx))?;
        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            events,
        })
    }

    pub fn open_document(&mut self, generation: u64, path: &Path) -> io::Result<()> {
        let path = path
            .to_str()
            .ok_or_else(|| invalid("UTF-8로 표현할 수 없는 경로"))?
            .to_string();
        self.send(&Request::Open { generation, path })
    }

    pub fn render(&mut self, request: RenderRequest) -> io::Result<()> {
        self.send(&Request::Render(request))
    }

    pub fn try_event(&self) -> Option<WorkerEvent> {
        self.events.try_recv().ok()
    }

    fn send(&mut self, request: &Request) -> io::Result<()> {
        write_request(&mut self.stdin, request)?;
        self.stdin.flush()
    }
}

impl Drop for RenderWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 응답 읽기 스레드. RGBA → egui 이미지 변환까지 여기서 한다 — 고배율에서는 수백 MB라 UI
/// 스레드에서 하면 그만큼 멈춘다. 텍스처 등록은 하지 않는다: 텍스처의 생성·해제를 메인
/// 스레드 프레임 흐름 안에 가둬야 "그린 텍스처를 같은 프레임에 해제" 크래시를 구조적으로
/// 막을 수 있다(texture_cache 모듈 문서). 결과가 올 때마다 화면을 깨운다.
fn read_responses(stdout: ChildStdout, sender: Sender<WorkerEvent>, ctx: egui::Context) {
    let mut reader = BufReader::with_capacity(1 << 20, stdout);
    loop {
        let event = match read_response(&mut reader) {
            Ok(Some(Response::Rendered {
                generation,
                page,
                target_width,
                width,
                height,
                rgba,
            })) => {
                let image =
                    egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
                WorkerEvent::Rendered {
                    generation,
                    page,
                    target_width,
                    image,
                }
            }
            Ok(Some(Response::Failed {
                generation,
                page,
                target_width,
                message,
            })) => WorkerEvent::Finished {
                generation,
                page,
                target_width,
                error: Some(message),
            },
            Ok(Some(Response::Dropped {
                generation,
                page,
                target_width,
            })) => WorkerEvent::Finished {
                generation,
                page,
                target_width,
                error: None,
            },
            Ok(None) | Err(_) => {
                let _ = sender.send(WorkerEvent::Died);
                ctx.request_repaint();
                return;
            }
        };
        if sender.send(event).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// 보조 프로세스 진입점(main.rs). 종료 코드를 돌려준다.
pub fn run_worker_process() -> i32 {
    let Some(engine) = crate::app::create_engine() else {
        eprintln!("render-worker: pdfium 라이브러리를 찾지 못했습니다");
        return 2;
    };
    let (sender, requests) = mpsc::channel();
    std::thread::spawn(move || {
        let mut input = BufReader::new(io::stdin());
        // EOF(메인 프로세스 종료)나 읽기 오류면 채널이 닫혀 worker_loop가 끝난다.
        while let Ok(Some(request)) = read_request(&mut input) {
            if sender.send(request).is_err() {
                break;
            }
        }
    });
    match worker_loop(engine, requests, io::stdout().lock()) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("render-worker: {err}");
            1
        }
    }
}

/// 요청을 받아 한 번에 하나씩 렌더링한다. pdfium은 이 함수를 도는 스레드에서만 쓴다.
fn worker_loop(engine: PdfEngine, requests: Receiver<Request>, output: impl Write) -> io::Result<()> {
    let mut output = BufWriter::with_capacity(1 << 20, output);
    let mut document: Result<PdfDocument<'static>, String> = Err("열린 문서가 없음".to_string());
    let mut generation = 0_u64;
    let mut queue = VecDeque::new();

    loop {
        // 대기열이 비었으면 다음 요청을 기다리고, 이미 도착해 있는 요청은 전부 가져온다 —
        // 그래야 그사이 화면이 바뀌어 쓸모없어진 요청을 렌더링 전에 걸러낼 수 있다.
        if queue.is_empty() {
            match requests.recv() {
                Ok(request) => accept(&engine, request, &mut document, &mut generation, &mut queue),
                Err(_) => return Ok(()),
            }
        }
        while let Ok(request) = requests.try_recv() {
            accept(&engine, request, &mut document, &mut generation, &mut queue);
        }

        let (dropped, next) = take_next(&mut queue, generation);
        for request in dropped {
            write_response(
                &mut output,
                &Response::Dropped {
                    generation: request.generation,
                    page: request.page,
                    target_width: request.target_width,
                },
            )?;
        }
        if let Some(request) = next {
            write_response(&mut output, &render_page(document.as_ref(), &request))?;
        }
        output.flush()?;
    }
}

fn accept(
    engine: &PdfEngine,
    request: Request,
    document: &mut Result<PdfDocument<'static>, String>,
    generation: &mut u64,
    queue: &mut VecDeque<RenderRequest>,
) {
    match request {
        Request::Open {
            generation: next,
            path,
        } => {
            // 새 문서를 열기 전에 옛 문서를 먼저 닫는다(메모리 두 벌 방지).
            *document = Err(String::new());
            *document = engine
                .open_document(Path::new(&path))
                .map_err(|err| format!("문서 열기 실패: {err}"));
            *generation = next;
        }
        Request::Render(render) => queue.push_back(render),
    }
}

/// 대기열에서 이번에 렌더링할 요청 하나를 고르고 버릴 요청들을 돌려준다 — 다른 문서 세대의
/// 요청과, 최신 epoch보다 오래된(이미 화면이 바뀐) 요청은 렌더링하지 않는다.
fn take_next(
    queue: &mut VecDeque<RenderRequest>,
    generation: u64,
) -> (Vec<RenderRequest>, Option<RenderRequest>) {
    let newest_epoch = queue
        .iter()
        .filter(|request| request.generation == generation)
        .map(|request| request.epoch)
        .max();
    let mut dropped = Vec::new();
    queue.retain(|request| {
        let keep = request.generation == generation && Some(request.epoch) == newest_epoch;
        if !keep {
            dropped.push(*request);
        }
        keep
    });
    (dropped, queue.pop_front())
}

fn render_page(document: Result<&PdfDocument<'static>, &String>, request: &RenderRequest) -> Response {
    let result = (|| -> Result<(u32, u32, Vec<u8>), String> {
        let document = document.map_err(|message| message.clone())?;
        let index = request
            .page
            .checked_sub(1)
            .ok_or_else(|| "페이지 번호 0".to_string())?;
        let page = document
            .pages()
            .get(index as PdfPageIndex)
            .map_err(|err| format!("페이지 조회 실패: {err}"))?;
        let bitmap = page
            .render_with_config(&PdfRenderConfig::new().set_target_width(request.render_width))
            .map_err(|err| format!("페이지 렌더링 실패: {err}"))?;
        let (width, height) = (bitmap.width().max(0) as u32, bitmap.height().max(0) as u32);
        let rgba = bitmap.as_rgba_bytes();
        // 크기가 0이거나 버퍼 길이가 w×h×4와 다르면 보내지 않는다 — 0 크기 텍스처는 wgpu가
        // 패닉하고(release는 panic=abort라 앱 종료), 길이가 다르면 파이프 프레임이 어긋난다.
        if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
            return Err(format!(
                "비정상 비트맵: {width}×{height}, 버퍼 {}바이트",
                rgba.len()
            ));
        }
        Ok((width, height, rgba))
    })();
    match result {
        Ok((width, height, rgba)) => Response::Rendered {
            generation: request.generation,
            page: request.page,
            target_width: request.target_width,
            width,
            height,
            rgba,
        },
        Err(message) => Response::Failed {
            generation: request.generation,
            page: request.page,
            target_width: request.target_width,
            message,
        },
    }
}

// ---- 이진 프레임 (리틀 엔디언) ----
// 요청:  [1] generation:u64 path:text
//        [2] generation:u64 epoch:u64 page:u32 target_width:i32 render_width:i32
// 응답:  [1] generation:u64 page:u32 target_width:i32 width:u32 height:u32 rgba:[u8; w*h*4]
//        [2] generation:u64 page:u32 target_width:i32 message:text
//        [3] generation:u64 page:u32 target_width:i32
// text = len:u32 + UTF-8 바이트

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn read_array<const N: usize>(reader: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    Ok(u32::from_le_bytes(read_array(reader)?))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    Ok(u64::from_le_bytes(read_array(reader)?))
}

fn read_i32(reader: &mut impl Read) -> io::Result<i32> {
    Ok(i32::from_le_bytes(read_array(reader)?))
}

fn read_text(reader: &mut impl Read) -> io::Result<String> {
    let len = read_u32(reader)? as usize;
    if len > MAX_TEXT_BYTES {
        return Err(invalid("텍스트 필드가 너무 김"));
    }
    let mut bytes = vec![0_u8; len];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| invalid("UTF-8이 아닌 텍스트"))
}

fn write_text(writer: &mut impl Write, text: &str) -> io::Result<()> {
    let mut end = text.len().min(MAX_TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    writer.write_all(&(end as u32).to_le_bytes())?;
    writer.write_all(&text.as_bytes()[..end])
}

/// 프레임 첫 바이트. 여기서 EOF면 상대가 정상 종료한 것이라 None.
fn read_tag(reader: &mut impl Read) -> io::Result<Option<u8>> {
    let mut tag = [0_u8; 1];
    loop {
        match reader.read(&mut tag) {
            Ok(0) => return Ok(None),
            Ok(_) => return Ok(Some(tag[0])),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}

fn write_request(writer: &mut impl Write, request: &Request) -> io::Result<()> {
    match request {
        Request::Open { generation, path } => {
            writer.write_all(&[TAG_OPEN])?;
            writer.write_all(&generation.to_le_bytes())?;
            write_text(writer, path)
        }
        Request::Render(render) => {
            writer.write_all(&[TAG_RENDER])?;
            writer.write_all(&render.generation.to_le_bytes())?;
            writer.write_all(&render.epoch.to_le_bytes())?;
            writer.write_all(&render.page.to_le_bytes())?;
            writer.write_all(&render.target_width.to_le_bytes())?;
            writer.write_all(&render.render_width.to_le_bytes())
        }
    }
}

fn read_request(reader: &mut impl Read) -> io::Result<Option<Request>> {
    let Some(tag) = read_tag(reader)? else {
        return Ok(None);
    };
    let request = match tag {
        TAG_OPEN => Request::Open {
            generation: read_u64(reader)?,
            path: read_text(reader)?,
        },
        TAG_RENDER => Request::Render(RenderRequest {
            generation: read_u64(reader)?,
            epoch: read_u64(reader)?,
            page: read_u32(reader)?,
            target_width: read_i32(reader)?,
            render_width: read_i32(reader)?,
        }),
        other => return Err(invalid(&format!("알 수 없는 요청 태그 {other}"))),
    };
    Ok(Some(request))
}

fn write_response(writer: &mut impl Write, response: &Response) -> io::Result<()> {
    match response {
        Response::Rendered {
            generation,
            page,
            target_width,
            width,
            height,
            rgba,
        } => {
            writer.write_all(&[TAG_RENDERED])?;
            writer.write_all(&generation.to_le_bytes())?;
            writer.write_all(&page.to_le_bytes())?;
            writer.write_all(&target_width.to_le_bytes())?;
            writer.write_all(&width.to_le_bytes())?;
            writer.write_all(&height.to_le_bytes())?;
            writer.write_all(rgba)
        }
        Response::Failed {
            generation,
            page,
            target_width,
            message,
        } => {
            writer.write_all(&[TAG_FAILED])?;
            writer.write_all(&generation.to_le_bytes())?;
            writer.write_all(&page.to_le_bytes())?;
            writer.write_all(&target_width.to_le_bytes())?;
            write_text(writer, message)
        }
        Response::Dropped {
            generation,
            page,
            target_width,
        } => {
            writer.write_all(&[TAG_DROPPED])?;
            writer.write_all(&generation.to_le_bytes())?;
            writer.write_all(&page.to_le_bytes())?;
            writer.write_all(&target_width.to_le_bytes())
        }
    }
}

fn read_response(reader: &mut impl Read) -> io::Result<Option<Response>> {
    let Some(tag) = read_tag(reader)? else {
        return Ok(None);
    };
    let generation = read_u64(reader)?;
    let page = read_u32(reader)?;
    let target_width = read_i32(reader)?;
    let response = match tag {
        TAG_RENDERED => {
            let width = read_u32(reader)?;
            let height = read_u32(reader)?;
            if width == 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS {
                return Err(invalid("비트맵 크기가 비정상"));
            }
            let mut rgba = vec![0_u8; width as usize * height as usize * 4];
            reader.read_exact(&mut rgba)?;
            Response::Rendered {
                generation,
                page,
                target_width,
                width,
                height,
                rgba,
            }
        }
        TAG_FAILED => Response::Failed {
            generation,
            page,
            target_width,
            message: read_text(reader)?,
        },
        TAG_DROPPED => Response::Dropped {
            generation,
            page,
            target_width,
        },
        other => return Err(invalid(&format!("알 수 없는 응답 태그 {other}"))),
    };
    Ok(Some(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn render_request(generation: u64, epoch: u64, page: u32) -> RenderRequest {
        RenderRequest {
            generation,
            epoch,
            page,
            target_width: 1800,
            render_width: 1700,
        }
    }

    #[test]
    fn requests_roundtrip() {
        let requests = [
            Request::Open {
                generation: 7,
                path: "/tmp/한글 경로/문서.pdf".to_string(),
            },
            Request::Render(render_request(7, 42, 358)),
        ];
        let mut bytes = Vec::new();
        for request in &requests {
            write_request(&mut bytes, request).unwrap();
        }
        let mut reader = Cursor::new(bytes);
        for request in &requests {
            assert_eq!(read_request(&mut reader).unwrap().as_ref(), Some(request));
        }
        // 프레임 경계에서의 EOF = 정상 종료.
        assert_eq!(read_request(&mut reader).unwrap(), None);
    }

    #[test]
    fn responses_roundtrip() {
        let responses = [
            Response::Rendered {
                generation: 3,
                page: 2,
                target_width: 900,
                width: 2,
                height: 1,
                rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
            },
            Response::Failed {
                generation: 3,
                page: 9,
                target_width: 900,
                message: "페이지 렌더링 실패".to_string(),
            },
            Response::Dropped {
                generation: 3,
                page: 4,
                target_width: -1,
            },
        ];
        let mut bytes = Vec::new();
        for response in &responses {
            write_response(&mut bytes, response).unwrap();
        }
        let mut reader = Cursor::new(bytes);
        for response in &responses {
            assert_eq!(read_response(&mut reader).unwrap().as_ref(), Some(response));
        }
        assert_eq!(read_response(&mut reader).unwrap(), None);
    }

    /// 프레임 중간에 끊기면(보조 프로세스가 죽음) 오류 — 정상 종료와 구분된다.
    #[test]
    fn truncated_frame_is_an_error() {
        let mut bytes = Vec::new();
        write_request(&mut bytes, &Request::Render(render_request(1, 1, 1))).unwrap();
        bytes.truncate(bytes.len() - 3);
        assert!(read_request(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn oversized_bitmap_header_is_rejected() {
        let mut bytes = vec![TAG_RENDERED];
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&1_i32.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_response(&mut Cursor::new(bytes)).is_err());
    }

    /// 화면이 바뀐 뒤(epoch 증가) 남은 옛 요청과 다른 문서의 요청은 렌더링하지 않고 버린다.
    #[test]
    fn stale_requests_are_dropped_before_rendering() {
        let mut queue: VecDeque<_> = [
            render_request(1, 5, 10), // 옛 문서
            render_request(2, 5, 11), // 옛 화면
            render_request(2, 6, 12), // 현재 화면 — 먼저 온 것부터
            render_request(2, 6, 13),
        ]
        .into();
        let (dropped, next) = take_next(&mut queue, 2);
        assert_eq!(dropped.iter().map(|r| r.page).collect::<Vec<_>>(), vec![10, 11]);
        assert_eq!(next.map(|r| r.page), Some(12));
        assert_eq!(queue.iter().map(|r| r.page).collect::<Vec<_>>(), vec![13]);

        let (dropped, next) = take_next(&mut VecDeque::new(), 2);
        assert!(dropped.is_empty() && next.is_none());
    }

    /// 실제 pdfium으로 보조 프로세스 루프를 돌린다: 옛 화면 요청은 Dropped, 최신 요청은 직접
    /// 렌더링한 것과 같은 크기의 비트맵으로 돌아온다. pdfium 라이브러리가 필요해 기본 제외:
    /// `PDFIUM_DYLIB_PATH=... cargo test -p ui --bins -- --ignored worker_loop`
    #[test]
    #[ignore]
    fn worker_loop_renders_latest_and_drops_stale() {
        let lib = std::path::PathBuf::from(std::env::var("PDFIUM_DYLIB_PATH").expect("PDFIUM_DYLIB_PATH"));
        let engine = PdfEngine::new_with_library_path(&lib).unwrap();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pdf-samples/SQ-main.pdf");

        let (sender, requests) = mpsc::channel();
        sender
            .send(Request::Open {
                generation: 1,
                path: path.to_str().unwrap().to_string(),
            })
            .unwrap();
        sender.send(Request::Render(RenderRequest { generation: 1, epoch: 1, page: 1, target_width: 800, render_width: 800 })).unwrap();
        sender.send(Request::Render(RenderRequest { generation: 1, epoch: 2, page: 2, target_width: 900, render_width: 800 })).unwrap();
        sender.send(Request::Render(RenderRequest { generation: 1, epoch: 2, page: 0, target_width: 900, render_width: 800 })).unwrap();
        drop(sender);

        let mut output = Vec::new();
        worker_loop(engine, requests, &mut output).unwrap();

        let mut reader = Cursor::new(output);
        let mut responses = Vec::new();
        while let Some(response) = read_response(&mut reader).unwrap() {
            responses.push(response);
        }
        assert_eq!(responses.len(), 3, "Dropped + Rendered + Failed");
        assert_eq!(responses[0], Response::Dropped { generation: 1, page: 1, target_width: 800 });

        let document = engine.open_document(&path).unwrap();
        let expected_page = document.pages().get(1).unwrap();
        let expected = expected_page
            .render_with_config(&PdfRenderConfig::new().set_target_width(800))
            .unwrap();
        match &responses[1] {
            Response::Rendered { page, target_width, width, height, rgba, .. } => {
                assert_eq!((*page, *target_width), (2, 900));
                assert_eq!((*width as i32, *height as i32), (expected.width(), expected.height()));
                assert_eq!(rgba.len(), (*width * *height * 4) as usize);
                assert_eq!(rgba, &expected.as_rgba_bytes());
            }
            other => panic!("Rendered 기대, 실제 {other:?}"),
        }
        assert!(matches!(&responses[2], Response::Failed { page: 0, .. }));
    }
}
