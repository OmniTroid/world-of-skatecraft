use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
// Local-folder mode: the page maps each picked file to a blob URL in `__benillaLocal`.
const DATA = '/wow/Data';
function local(url) {
    const l = globalThis.__benillaLocal;
    if (!l || !url.startsWith(DATA)) return undefined;
    const name = url.slice(DATA.length).replace(/^\/+/, '');
    return name === '' ? { list: Object.keys(l) } : (l[name] ?? null);
}
function get(method, url, range) {
    const x = new XMLHttpRequest();
    // A thread's worker runs from a blob: URL, which resolves no root-relative path.
    x.open(method, url.startsWith('/') ? location.origin + url : url, false);
    x.overrideMimeType('text/plain; charset=x-user-defined');
    if (range) x.setRequestHeader('Range', range);
    x.send();
    return x;
}
export function web_len(url) {
    const l = local(url);
    if (l !== undefined) return l?.size ?? -1;
    const x = get('HEAD', url);
    return x.status === 200 ? Number(x.getResponseHeader('Content-Length')) : -1;
}
export function web_range(url, start, end) {
    const l = local(url);
    if (l === null) return undefined;
    const x = get('GET', l?.url ?? url, `bytes=${start}-${end - 1}`);
    if (x.status !== 206 && x.status !== 200) return undefined;
    const s = x.responseText, off = x.status === 200 ? start : 0, n = Math.min(end - start, s.length - off);
    const out = new Uint8Array(Math.max(n, 0));
    for (let i = 0; i < out.length; i++) out[i] = s.charCodeAt(off + i) & 0xff;
    return out;
}
export function web_list(url) {
    const l = local(url);
    if (l?.list) return l.list;
    if (l !== undefined) return undefined;
    const x = get('GET', url.endsWith('/') ? url : url + '/');
    if (x.status !== 200) return undefined;
    try {
        return JSON.parse(x.responseText).filter(e => e.type === 'file').map(e => e.name);
    } catch (_) { return undefined; }
}
"#)]
extern "C" {
    fn web_len(url: &str) -> f64;
    fn web_range(url: &str, start: f64, end: f64) -> Option<js_sys::Uint8Array>;
    fn web_list(url: &str) -> Option<Vec<String>>;
}

/// Bytes per cached block: an MPQ read is a few sectors, so one request serves its neighbours.
const BLOCK: u64 = 256 * 1024;
/// Blocks kept, oldest evicted first.
const CACHED_BLOCKS: usize = 512;

#[derive(Default)]
struct Cache {
    blocks: HashMap<(Rc<str>, u64), Rc<Vec<u8>>>,
    order: VecDeque<(Rc<str>, u64)>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
    static LENS: RefCell<HashMap<String, u64>> = RefCell::default();
}

fn url_of(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn block(url: &Rc<str>, index: u64, len: u64) -> io::Result<Rc<Vec<u8>>> {
    let key = (url.clone(), index);
    if let Some(b) = CACHE.with(|c| c.borrow().blocks.get(&key).cloned()) {
        return Ok(b);
    }
    let start = index * BLOCK;
    let end = (start + BLOCK).min(len);
    let bytes = web_range(url, start as f64, end as f64)
        .ok_or_else(|| io::Error::other(format!("{url}: range {start}..{end} failed")))?
        .to_vec();
    let bytes = Rc::new(bytes);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.order.len() >= CACHED_BLOCKS {
            if let Some(old) = c.order.pop_front() {
                c.blocks.remove(&old);
            }
        }
        c.order.push_back(key.clone());
        c.blocks.insert(key, bytes.clone());
    });
    Ok(bytes)
}

/// A file on the page's origin, positioned like `std::fs::File`.
pub struct File {
    url: Rc<str>,
    len: u64,
    pos: u64,
}

// The web build runs one thread: the handles never cross one.
unsafe impl Send for File {}
unsafe impl Sync for File {}

impl File {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let url = url_of(path.as_ref());
        let len = match LENS.with(|l| l.borrow().get(&url).copied()) {
            Some(len) => len,
            None => {
                let len = web_len(&url);
                if len < 0.0 {
                    return Err(io::Error::new(io::ErrorKind::NotFound, url));
                }
                LENS.with(|l| l.borrow_mut().insert(url.clone(), len as u64));
                len as u64
            }
        };
        Ok(Self {
            url: url.into(),
            len,
            pos: 0,
        })
    }

    pub fn len(&self) -> u64 {
        self.len
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let index = self.pos / BLOCK;
        let b = block(&self.url, index, self.len)?;
        let at = (self.pos - index * BLOCK) as usize;
        let n = buf.len().min(b.len().saturating_sub(at));
        buf[..n].copy_from_slice(&b[at..at + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for File {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.pos)
    }
}

pub fn list(path: &Path) -> Option<Vec<String>> {
    web_list(&url_of(path))
}
