// SPDX-License-Identifier: MIT
//
// hedgehog: this file is not part of calamine 0.37.0.

//! A large worksheet or shared string table read on several threads, with exactly the result of
//! the one-thread read.
//!
//! The part is inflated on the calling thread and cut into chunks of about [`CHUNK`] bytes, each
//! ending right after a `</row>` (worksheets) or `</si>` (shared strings). Worker threads read the
//! chunks with calamine's own code (the same XML reader settings, `next_cell_formula_record`,
//! `read_string_with_bufs`), and the calling thread takes the results in chunk order.
//!
//! A chunk of a worksheet does not know the state the rows before it leave. A derived shared
//! formula whose anchor is not in its chunk is expanded when the chunk's turn comes, from the
//! anchors of the chunks before it (exactly the anchor the one-thread read has at that cell). A
//! chunk that needs the row index of the rows before it (a row without `r`) is read again in
//! order. Anything else unusual gives up (`None`) before any result is used, and the caller
//! reads the part on one thread: comments, CDATA, declarations or processing instructions after
//! the start tag (they could hide a boundary), a declared encoding other than UTF-8, a chunk that
//! does not end right after a row or string, an XML or value error anywhere (the one-thread read
//! then reports it), a part that ends early.

use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{mpsc, Mutex};

use memchr::memmem;
use quick_xml::events::Event;
use quick_xml::Reader as XmlReader;

use super::cells_reader::{
    next_cell_formula_record, read_sheet_head, CellState, ChunkFlags, SharedFormula,
    WorkbookContext,
};
use super::{configure, read_string_with_bufs, Xlsx, XlsxCellFormula, XlsxError};
use crate::utils::cached_zip_path;
use crate::Dimensions;

/// Bytes of XML per chunk.
const CHUNK: usize = 512 << 10;
/// No chunk boundary (or start tag) within this many bytes: give up.
const MAX_PENDING: usize = 64 << 20;
/// Shared string tables smaller than this (uncompressed) are read on one thread.
const MIN_PARALLEL_STRINGS: u64 = 2 << 20;

/// Parts read in parallel, parts that gave up after starting, chunks read again in order.
static PARTS: AtomicUsize = AtomicUsize::new(0);
static GAVE_UP: AtomicUsize = AtomicUsize::new(0);
static AGAIN: AtomicUsize = AtomicUsize::new(0);

/// (parts read in parallel, parts that started and gave up, chunks read again in order) since the
/// process started — for tests and benchmarks.
#[doc(hidden)]
pub fn parallel_read_stats() -> (usize, usize, usize) {
    (PARTS.load(Relaxed), GAVE_UP.load(Relaxed), AGAIN.load(Relaxed))
}

/// Counts a part another reader (ODS tables) read in parallel, or gave up on after starting.
pub(crate) fn count_part(done: bool) {
    if done { PARTS.fetch_add(1, Relaxed); } else { GAVE_UP.fetch_add(1, Relaxed); }
}

thread_local! {
    /// Test settings of this thread's reads: (threads, chunk bytes, smallest part read in
    /// parallel).
    static SETTINGS: std::cell::Cell<Option<(usize, usize, u64)>> = const { std::cell::Cell::new(None) };
}

/// For tests: reads on this thread use `threads` threads, chunks of about `chunk` bytes and the
/// parallel read for every part of at least `min_part` bytes (sheets and shared strings);
/// `None` restores the defaults.
#[doc(hidden)]
pub fn set_parallel_read_for_tests(settings: Option<(usize, usize, u64)>) {
    SETTINGS.with(|s| s.set(settings));
}

/// Threads for one parallel read: the available CPUs (CPU affinity respected), at most 4.
pub fn parallel_read_threads() -> usize {
    if let Some((threads, _, _)) = SETTINGS.with(|s| s.get()) {
        return threads;
    }
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4)
}

pub(crate) fn chunk_bytes() -> usize {
    SETTINGS.with(|s| s.get()).map_or(CHUNK, |(_, c, _)| c.max(1))
}

pub(crate) fn min_part(default: u64) -> u64 {
    SETTINGS.with(|s| s.get()).map_or(default, |(_, _, m)| m)
}

/// Reads until `buf` is full or the input ends; the number of bytes read.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// The part's bytes up to and including the first start tag named `tag` (and the bytes read
/// after it), once a `>` follows the first `<tag`; `None` past [`MAX_PENDING`] or at the end.
fn read_head(r: &mut impl Read, tag: &[u8], chunk: usize) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    loop {
        let old = buf.len();
        buf.resize(old + chunk, 0);
        let n = read_full(r, &mut buf[old..]).ok()?;
        buf.truncate(old + n);
        if let Some(i) = memmem::find(&buf, tag) {
            if memchr::memchr(b'>', &buf[i..]).is_some() {
                return Some(buf);
            }
        }
        if n == 0 || buf.len() >= MAX_PENDING {
            return None;
        }
    }
}

/// Cuts the rest of a part into chunks that end right after `end_tag`.
struct Splitter<R: Read> {
    r: R,
    pending: Vec<u8>,
    /// Bytes of `pending` already checked for `<!` and `<?`.
    scanned: usize,
    eof: bool,
    done: bool,
    end_tag: &'static [u8],
    chunk: usize,
}

impl<R: Read> Splitter<R> {
    fn new(r: R, pending: Vec<u8>, end_tag: &'static [u8], chunk: usize) -> Self {
        Splitter {
            r,
            pending,
            scanned: 0,
            eof: false,
            done: false,
            end_tag,
            chunk,
        }
    }

    /// The next chunk and whether it is the last one; `Ok(None)` after the last; `Err(())` to
    /// give up.
    fn next_chunk(&mut self) -> Result<Option<(Vec<u8>, bool)>, ()> {
        if self.done {
            return Ok(None);
        }
        let mut target = self.chunk;
        loop {
            while self.pending.len() < target && !self.eof {
                let old = self.pending.len();
                self.pending.resize(old + self.chunk, 0);
                let n = read_full(&mut self.r, &mut self.pending[old..]).map_err(|_| ())?;
                self.pending.truncate(old + n);
                self.eof = n < self.chunk;
            }
            let from = self.scanned.saturating_sub(1);
            let fresh = &self.pending[from..];
            if memmem::find(fresh, b"<!").is_some() || memmem::find(fresh, b"<?").is_some() {
                return Err(());
            }
            self.scanned = self.pending.len();
            if self.eof {
                self.done = true;
                return Ok(Some((std::mem::take(&mut self.pending), true)));
            }
            match memmem::rfind(&self.pending, self.end_tag) {
                Some(i) => {
                    // An exact copy: the jobs in flight hold no spare capacity, and `pending`
                    // keeps its buffer.
                    let cut = i + self.end_tag.len();
                    let chunk = self.pending[..cut].to_vec();
                    self.pending.drain(..cut);
                    self.scanned = self.pending.len();
                    return Ok(Some((chunk, false)));
                }
                None if self.pending.len() >= MAX_PENDING => return Err(()),
                None => target = self.pending.len() + self.chunk,
            }
        }
    }
}

/// What the in-order step decides after a chunk.
pub(crate) enum Flow {
    Continue,
    /// The part is read (its end tag was in this chunk).
    Done,
    GiveUp,
}

/// Runs `work` on `threads` threads over the chunks `next` gives (with whether each is the last;
/// `Err` to give up) and hands each result to `merge`
/// in chunk order, with whether it was the last chunk. At most `2 × threads` chunks are ahead of
/// `merge`. `Ok(true)`: `merge` said [`Flow::Done`]; `Ok(false)`: give up.
pub(crate) fn run_chunks<O, E>(
    next_chunk: &mut dyn FnMut() -> Result<Option<(Vec<u8>, bool)>, ()>,
    threads: usize,
    work: &(dyn Fn(usize, Vec<u8>) -> O + Sync),
    merge: &mut dyn FnMut(O, bool) -> Result<Flow, E>,
) -> Result<bool, E>
where
    O: Send,
{
    let (job_tx, job_rx) = mpsc::sync_channel::<(usize, Vec<u8>)>(threads);
    let job_rx = Mutex::new(job_rx);
    let (res_tx, res_rx) = mpsc::channel::<(usize, O)>();
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|sc| {
        for _ in 0..threads {
            let res_tx = res_tx.clone();
            let (job_rx, stop) = (&job_rx, &stop);
            sc.spawn(move || loop {
                let job = match job_rx.lock() {
                    Ok(rx) => rx.recv(),
                    Err(_) => break,
                };
                let Ok((i, bytes)) = job else { break };
                if stop.load(Relaxed) || res_tx.send((i, work(i, bytes))).is_err() {
                    break;
                }
            });
        }
        drop(res_tx);
        let result = (|| {
            // Results that came before their turn, by chunk index.
            let mut early: Vec<Option<O>> = Vec::new();
            let mut lasts: Vec<bool> = Vec::new();
            let mut next = 0usize;
            // Takes a result, then merges every result whose turn has come.
            let mut take = |i: usize,
                            o: O,
                            early: &mut Vec<Option<O>>,
                            next: &mut usize,
                            lasts: &[bool]|
             -> Result<Option<bool>, E> {
                if early.len() <= i {
                    early.resize_with(i + 1, || None);
                }
                early[i] = Some(o);
                while let Some(o) = early.get_mut(*next).and_then(Option::take) {
                    let last = lasts[*next];
                    *next += 1;
                    match merge(o, last)? {
                        Flow::Continue => {}
                        Flow::Done => return Ok(Some(true)),
                        Flow::GiveUp => return Ok(Some(false)),
                    }
                }
                Ok(None)
            };
            let mut finished = false;
            while !finished {
                // Not too far ahead of the merge.
                while lasts.len() >= next + 2 * threads {
                    let Ok((i, o)) = res_rx.recv() else {
                        return Ok(false);
                    };
                    if let Some(done) = take(i, o, &mut early, &mut next, &lasts)? {
                        return Ok(done);
                    }
                }
                match next_chunk() {
                    Err(()) => return Ok(false),
                    Ok(None) => finished = true,
                    Ok(Some((bytes, last))) => {
                        if job_tx.send((lasts.len(), bytes)).is_err() {
                            return Ok(false);
                        }
                        lasts.push(last);
                        finished = last;
                    }
                }
                while let Ok((i, o)) = res_rx.try_recv() {
                    if let Some(done) = take(i, o, &mut early, &mut next, &lasts)? {
                        return Ok(done);
                    }
                }
            }
            while next < lasts.len() {
                let Ok((i, o)) = res_rx.recv() else {
                    return Ok(false);
                };
                if let Some(done) = take(i, o, &mut early, &mut next, &lasts)? {
                    return Ok(done);
                }
            }
            Ok(false) // no end tag: the part ends early
        })();
        // Workers skip the chunks still queued and end.
        stop.store(true, Relaxed);
        drop(job_tx);
        result
    })
}

/// One chunk of sheet data, read.
struct SheetChunk<'s, T> {
    cells: Vec<T>,
    /// Records with a derived shared formula whose anchor was not in the chunk: where they go
    /// in `cells` (before the record there), the record without its formula, the shared index.
    unresolved: Vec<(usize, XlsxCellFormula<'s>, usize)>,
    flags: ChunkFlags,
    row_index: u32,
    formulas: Vec<Option<SharedFormula>>,
    /// The bytes, kept when the chunk must be read again in order.
    bytes: Option<Vec<u8>>,
}

/// Reads one chunk; `start`: the row index and shared formulas the chunks before it leave (the
/// chunk is then read in order, as the one-thread read would), `None` to read it on its own.
fn read_sheet_chunk<'s, T, C>(
    bytes: Vec<u8>,
    ctx: &WorkbookContext<'s>,
    convert: &C,
    start: Option<(u32, Vec<Option<SharedFormula>>)>,
) -> Result<SheetChunk<'s, T>, XlsxError>
where
    C: for<'x> Fn(XlsxCellFormula<'x>) -> Option<T>,
{
    let in_order = start.is_some();
    let mut st = CellState::new(Some(ChunkFlags {
        row_known: in_order,
        in_order,
        ..ChunkFlags::default()
    }));
    if let Some((row_index, formulas)) = start {
        st.row_index = row_index;
        st.formulas = formulas;
    }
    // One allocation of about the chunk's size, which the next chunks reuse once it is freed.
    let mut cells = Vec::with_capacity(bytes.len() / 48);
    let mut unresolved = Vec::new();
    {
        // The end tag of `sheetData` must have a start tag to match.
        let mut xml = XmlReader::from_reader(Read::chain(&b"<sheetData>"[..], bytes.as_slice()));
        configure(&mut xml);
        while let Some(rec) = next_cell_formula_record(&mut xml, &mut st, ctx, true)? {
            match rec.unresolved_shared().filter(|_| !in_order) {
                Some(si) => unresolved.push((cells.len(), rec.into_formula_record(), si)),
                None => {
                    if let Some(t) = convert(rec.into_formula_record()) {
                        cells.push(t);
                    }
                }
            }
            if !in_order && st.chunk.is_some_and(|c| c.needs_context) {
                break;
            }
        }
    }
    let flags = st.chunk.unwrap_or_default();
    let again = !in_order && flags.needs_context;
    if again {
        cells.clear();
        unresolved.clear();
    }
    Ok(SheetChunk {
        cells,
        unresolved,
        flags,
        row_index: st.row_index,
        formulas: st.formulas,
        bytes: again.then_some(bytes),
    })
}

/// One chunk of a shared string table, read: its strings, whether it reached `</sst>`, and
/// whether it ended right after a string otherwise.
fn read_strings_chunk(bytes: &[u8]) -> Result<(Vec<String>, bool, bool), XlsxError> {
    // The end tag of `sst` must have a start tag to match.
    let mut xml = XmlReader::from_reader(Read::chain(&b"<sst>"[..], bytes));
    configure(&mut xml);
    let mut buf = Vec::with_capacity(1024);
    let mut str_buf = Vec::with_capacity(1024);
    let mut str_val_buf = Vec::with_capacity(1024);
    let mut out = Vec::new();
    let mut after_string = false;
    loop {
        buf.clear();
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "si" => {
                if let Some(s) =
                    read_string_with_bufs(&mut xml, e.name(), &mut str_buf, &mut str_val_buf)?
                {
                    out.push(s);
                }
                after_string = true;
                continue;
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "sst" => return Ok((out, true, false)),
            Ok(Event::Eof) => return Ok((out, false, after_string)),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => (),
        }
        after_string = false;
    }
}

impl<RS: Read + std::io::Seek> Xlsx<RS> {
    /// hedgehog: the records of sheet `name` exactly as [`super::XlsxCellReader`]'s
    /// `next_cell_with_formula` returns them, passed through `convert` (`None` drops a record),
    /// read on `threads` threads. `sink` takes them in chunks, in order, as soon as each chunk's
    /// turn comes (while the next chunks are read), and may stop the read with its error. The
    /// sheet's declared dimensions.
    ///
    /// `Ok(None)`: not read this way (fewer than 2 threads, a part smaller than `min_size`
    /// bytes uncompressed, or anything unusual, see the module notes; `sink` may have taken
    /// chunks already): read the sheet with [`Xlsx::worksheet_cells_reader`], which also reports
    /// any error.
    pub fn worksheet_cells_parallel<T, C, E>(
        &mut self,
        name: &str,
        threads: usize,
        min_size: u64,
        convert: C,
        sink: &mut dyn FnMut(Vec<T>) -> Result<(), E>,
    ) -> Result<Option<Dimensions>, E>
    where
        T: Send,
        C: for<'x> Fn(XlsxCellFormula<'x>) -> Option<T> + Sync,
    {
        if threads < 2 {
            return Ok(None);
        }
        let Some((_, path)) = self.sheets.iter().find(|(n, _)| n == name) else {
            return Ok(None);
        };
        let ctx = WorkbookContext {
            strings: &self.strings,
            formats: &self.formats,
            is_1904: self.is_1904,
        };
        let Ok(mut part) = self.zip.by_name(cached_zip_path(&self.zip_path_cache, path)) else {
            return Ok(None);
        };
        if part.size() < min_part(min_size) {
            return Ok(None);
        }
        let chunk = chunk_bytes();
        let Some(mut head) = read_head(&mut part, b"<sheetData", chunk) else {
            return Ok(None);
        };
        let (dims, body) = {
            let mut xml = XmlReader::from_reader(head.as_slice());
            configure(&mut xml);
            let Ok(dims) = read_sheet_head(&mut xml, &mut Vec::new()) else {
                return Ok(None);
            };
            if xml.encoding() != encoding_rs::UTF_8 {
                return Ok(None);
            }
            let body = xml.buffer_position() as usize;
            // `<sheetData/>`: the reader made up the end tag.
            if body < 2 || head.get(body - 1) != Some(&b'>') || head[body - 2] == b'/' {
                return Ok(None);
            }
            (dims, body)
        };
        let rest = head.split_off(body);
        drop(head);
        let mut split = Splitter::new(part, rest, b"</row>", chunk);
        GAVE_UP.fetch_add(1, Relaxed); // until it is done

        let work = |i: usize, bytes: Vec<u8>| {
            read_sheet_chunk(bytes, &ctx, &convert, (i == 0).then(|| (0, Vec::new())))
        };
        // In order: the state each chunk leaves, chunks read again where they need it.
        let mut formulas: Vec<Option<SharedFormula>> = Vec::new();
        let mut row_index = 0u32;
        let mut merge = |r: Result<SheetChunk<'_, T>, XlsxError>, last: bool| -> Result<Flow, E> {
            let Ok(mut c) = r else { return Ok(Flow::GiveUp) };
            if let Some(bytes) = c.bytes.take() {
                AGAIN.fetch_add(1, Relaxed);
                let start = Some((row_index, std::mem::take(&mut formulas)));
                c = match read_sheet_chunk(bytes, &ctx, &convert, start) {
                    Ok(c) => c,
                    Err(_) => return Ok(Flow::GiveUp),
                };
            }
            // Derived shared formulas anchored before the chunk: the anchor the chunks before
            // it leave (none in the chunk came before the cell, or it would have been used).
            if !c.unresolved.is_empty() {
                let mut cells = Vec::with_capacity(c.cells.len() + c.unresolved.len());
                let mut rest = std::mem::take(&mut c.cells).into_iter();
                let mut at = 0;
                for (i, mut rec, si) in std::mem::take(&mut c.unresolved) {
                    cells.extend(rest.by_ref().take(i - at));
                    at = i;
                    if let Some(t) = formulas.get(si).and_then(Option::as_ref) {
                        match t.expand(rec.pos) {
                            Ok(f) => rec.formula = Some(f),
                            Err(_) => return Ok(Flow::GiveUp),
                        }
                    }
                    if let Some(t) = convert(rec) {
                        cells.push(t);
                    }
                }
                cells.extend(rest);
                c.cells = cells;
            }
            if c.flags.in_order {
                formulas = c.formulas;
                row_index = c.row_index;
            } else {
                for (si, f) in c.formulas.into_iter().enumerate() {
                    if let Some(f) = f {
                        if formulas.len() <= si {
                            formulas.resize(si + 1, None);
                        }
                        formulas[si] = Some(f);
                    }
                }
                if c.flags.row_known {
                    row_index = c.row_index;
                }
            }
            // The one-thread read stops at `</sheetData>`; a chunk without it ends right after
            // a row, and the last one has it (else the part ends early: an error).
            let ended = !c.flags.at_eof;
            if !ended && (last || !c.flags.last_row_end) {
                return Ok(Flow::GiveUp);
            }
            sink(c.cells)?;
            Ok(if ended { Flow::Done } else { Flow::Continue })
        };
        if !run_chunks(&mut || split.next_chunk(), threads, &work, &mut merge)? {
            return Ok(None);
        }
        GAVE_UP.fetch_sub(1, Relaxed);
        PARTS.fetch_add(1, Relaxed);
        Ok(Some(dims))
    }

    /// hedgehog: [`Xlsx::read_shared_strings`] on several threads for a large table: `true`
    /// when the strings were read (exactly as the one-thread read reads them), `false` to read
    /// them on one thread (which also reports any error).
    pub(super) fn read_shared_strings_parallel(&mut self, threads: usize) -> bool {
        if threads < 2 {
            return false;
        }
        let path = format!("{}sharedStrings.xml", self.xl_path);
        let Ok(mut part) = self.zip.by_name(cached_zip_path(&self.zip_path_cache, &path)) else {
            return false;
        };
        if part.size() < min_part(MIN_PARALLEL_STRINGS) {
            return false;
        }
        let chunk = chunk_bytes();
        let Some(mut head) = read_head(&mut part, b"<sst", chunk) else {
            return false;
        };
        let mut reserve = None;
        let body = {
            let mut xml = XmlReader::from_reader(head.as_slice());
            configure(&mut xml);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match xml.read_event_into(&mut buf) {
                    Ok(Event::Start(e)) if e.local_name().as_ref() == "sst" => {
                        match crate::attrs::RawAttributes::raw_attr(&e, "uniqueCount") {
                            Ok(Some(count)) => {
                                if let Ok(n) = atoi_simd::parse::<usize, true, false>(count.as_bytes())
                                {
                                    reserve = Some(n);
                                }
                            }
                            Ok(None) => {}
                            Err(_) => return false,
                        }
                        break;
                    }
                    Ok(Event::Eof) | Err(_) => return false,
                    _ => (),
                }
            }
            if xml.encoding() != encoding_rs::UTF_8 {
                return false;
            }
            let body = xml.buffer_position() as usize;
            if body < 2 || head.get(body - 1) != Some(&b'>') || head[body - 2] == b'/' {
                return false;
            }
            body
        };
        let rest = head.split_off(body);
        drop(head);
        let mut split = Splitter::new(part, rest, b"</si>", chunk);
        GAVE_UP.fetch_add(1, Relaxed); // until it is done
        let work = |_: usize, bytes: Vec<u8>| read_strings_chunk(&bytes);
        let mut chunks = Vec::new();
        let mut merge = |r: Result<(Vec<String>, bool, bool), XlsxError>, last: bool| {
            let Ok((strings, ended, after_string)) = r else {
                return Ok::<_, ()>(Flow::GiveUp);
            };
            if !ended && (last || !after_string) {
                return Ok(Flow::GiveUp);
            }
            chunks.push(strings);
            Ok(if ended { Flow::Done } else { Flow::Continue })
        };
        if run_chunks(&mut || split.next_chunk(), threads, &work, &mut merge) != Ok(true) {
            return false;
        }
        if let Some(n) = reserve {
            // the declared count is untrusted, as in the one-thread read
            self.strings.reserve(n.min(1 << 16));
        }
        self.strings.reserve(chunks.iter().map(Vec::len).sum());
        for c in chunks {
            self.strings.extend(c);
        }
        GAVE_UP.fetch_sub(1, Relaxed);
        PARTS.fetch_add(1, Relaxed);
        true
    }
}
