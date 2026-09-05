//! `pdfrum serve --stdio`: a session for agents.
//!
//! One process, each document parsed once and kept under a small integer
//! id, and the commands as JSON-RPC 2.0 methods over stdin and stdout —
//! one request and one response per line — with the same JSON shapes the
//! commands' `--json` prints. The method table is `rpc::methods`. Stdout
//! carries responses and nothing else; stderr is silent unless
//! `--verbose`, which logs one line per request. `--mcp` speaks the Model
//! Context Protocol's surface (`initialize`, `tools/list`, `tools/call`)
//! over the same methods, so an agent host can discover `pdfrum` as a
//! tool set.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use pdfrum::{Document, FindOptions, Rect, RenderSession};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::rpc::{self, Error};
use crate::term::{base64, unbase64};
use crate::{cmd, out, pages};

/// What `serve` was asked for.
pub struct Options {
    /// The Model Context Protocol's methods instead of the plain ones.
    pub mcp: bool,
    /// How many documents may be open at once.
    pub max_docs: usize,
}

/// The MCP protocol revision this server answers `initialize` with.
const MCP_VERSION: &str = "2024-11-05";

/// An open document and what the reports call it: the path it was opened
/// from, or `-` for bytes.
struct Open {
    name: String,
    doc: Document,
}

/// The state between requests.
struct Session {
    docs: BTreeMap<u64, Open>,
    next_id: u64,
    max_docs: usize,
    /// The `--password`, for an `open` without one.
    password: Option<String>,
    /// One render cache for the session: glyphs and images survive between
    /// pages and documents.
    render: RenderSession,
    /// Set by `shutdown`; the loop ends after its reply.
    stopping: bool,
}

/// The loop: a line in, a line out, until `shutdown` or the end of stdin.
pub fn run(options: &Options, password: Option<&str>) -> Result<ExitCode> {
    // Stdout is the wire; stderr says nothing unless asked.
    if !out::verbose() {
        out::set_verbosity(out::Verbosity::Quiet);
    }
    let mut session = Session {
        docs: BTreeMap::new(),
        next_id: 0,
        max_docs: options.max_docs,
        password: password.map(str::to_owned),
        render: RenderSession::new(),
        stopping: false,
    };
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.context("cannot read stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        let request = match rpc::parse(&line) {
            Ok(request) => request,
            Err(err) => {
                reply(&rpc::failure(&Value::Null, &err));
                continue;
            }
        };
        let outcome = if options.mcp {
            session.mcp(&request.method, request.params)
        } else {
            session.call(&request.method, request.params)
        };
        if out::verbose() {
            match &outcome {
                Ok(_) => eprintln!("pdfrum: serve: {}: ok", request.method),
                Err(e) => eprintln!(
                    "pdfrum: serve: {}: {} ({})",
                    request.method, e.message, e.code
                ),
            }
        }
        if let Some(id) = request.id {
            reply(&match outcome {
                Ok(result) => rpc::response(&id, &result),
                Err(err) => rpc::failure(&id, &err),
            });
        }
        if session.stopping {
            break;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// One response line: compact JSON and a newline.
fn reply(value: &Value) {
    out::write_all(format_args!("{value}\n"));
}

// ---- params ---------------------------------------------------------------

/// The params as the method's struct; a missing or unknown field is
/// -32602 with serde's reason. No params at all is an empty object.
fn params<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    let value = if value.is_null() {
        Value::Object(Map::new())
    } else {
        value
    };
    serde_json::from_value(value).map_err(|e| Error::invalid_params(format!("invalid params: {e}")))
}

/// A command's outcome as the method's: -32000 with the CLI's error line.
fn failed<T>(result: Result<T>) -> Result<T, Error> {
    result.map_err(|e| Error::failed(&e))
}

/// A report as the result.
fn value(report: &impl serde::Serialize) -> Result<Value, Error> {
    serde_json::to_value(report)
        .map_err(|e| Error::failed(&anyhow::Error::from(e).context("cannot encode JSON")))
}

/// The open document `id` names.
fn doc_of(docs: &BTreeMap<u64, Open>, id: u64) -> Result<&Open, Error> {
    docs.get(&id).ok_or_else(|| {
        Error::invalid_params(format!("no document is open as {id}; `open` gives the id"))
    })
}

/// `spec` checked against the document before any work is done, so a bad
/// selection is -32602 and not a failure half-way.
fn selection(doc: &Document, spec: Option<&str>) -> Result<(), Error> {
    pages::select(spec, doc.page_count())
        .map(drop)
        .map_err(|e| Error::invalid_params(out::error_line(&e)))
}

/// A 1-based page as the 0-based index, checked against the document.
fn page_index(doc: &Document, page: u32) -> Result<u32, Error> {
    let count = doc.page_count();
    if page == 0 || page > count {
        return Err(Error::invalid_params(format!(
            "page {page} is not in 1..={count}"
        )));
    }
    Ok(page - 1)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenParams {
    file: Option<String>,
    bytes_base64: Option<String>,
    password: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocParams {
    doc: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DoctorParams {
    doc: u64,
    #[serde(default)]
    scan_all: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PagesParams {
    doc: u64,
    pages: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextParams {
    doc: u64,
    pages: Option<String>,
    #[serde(default)]
    layout: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImagesParams {
    doc: u64,
    pages: Option<String>,
    #[serde(default)]
    all: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageParams {
    doc: u64,
    index: usize,
    pages: Option<String>,
    #[serde(default)]
    all: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchParams {
    doc: u64,
    needle: String,
    #[serde(default)]
    ignore_case: bool,
    pages: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderParams {
    doc: u64,
    page: u32,
    dpi: Option<f64>,
    scale: Option<f64>,
    annotations: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FillParams {
    doc: u64,
    values: Map<String, Value>,
    #[serde(default)]
    deterministic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectParams {
    doc: u64,
    num: u32,
    #[serde(default, rename = "gen")]
    generation: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffParams {
    doc: u64,
    other: u64,
    #[serde(default)]
    visual: bool,
    dpi: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SliceParams {
    doc: u64,
    pages: Option<String>,
    rotate: Option<i32>,
    crop: Option<[f64; 4]>,
    #[serde(default)]
    deterministic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MergeParams {
    docs: Vec<u64>,
    #[serde(default)]
    deterministic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MetadataParams {
    doc: u64,
    title: Option<String>,
    author: Option<String>,
    subject: Option<String>,
    keywords: Option<String>,
    creator: Option<String>,
    #[serde(default)]
    clear: Vec<String>,
    #[serde(default)]
    deterministic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteParams {
    doc: u64,
    pages: String,
    #[serde(default)]
    deterministic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RotateParams {
    doc: u64,
    pages: Option<String>,
    by: i32,
    #[serde(default)]
    deterministic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachAddParams {
    doc: u64,
    name: String,
    bytes_base64: String,
    description: Option<String>,
    mime: Option<String>,
    #[serde(default)]
    deterministic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachRemoveParams {
    doc: u64,
    names: Vec<String>,
    #[serde(default)]
    deterministic: bool,
}

/// The mark of a stamp — `position`, `opacity`, `angle` — as both stamps
/// take them, `position` parsed as the command line parses `--position`.
fn mark(
    position: Option<&str>,
    opacity: Option<f32>,
    angle: Option<f64>,
) -> Result<cmd::stamp::Mark, Error> {
    let position = match position {
        Some(name) => <cmd::stamp::Position as clap::ValueEnum>::from_str(name, false)
            .map_err(|_| {
                Error::invalid_params(format!(
                    "position {name:?} is not one of center, top-left, top-right, bottom-left, bottom-right"
                ))
            })?,
        None => cmd::stamp::Position::Center,
    };
    Ok(cmd::stamp::Mark {
        position,
        opacity: opacity.unwrap_or(1.0),
        angle: angle.unwrap_or(0.0),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StampTextParams {
    doc: u64,
    text: String,
    position: Option<String>,
    opacity: Option<f32>,
    angle: Option<f64>,
    size: Option<f32>,
    color: Option<String>,
    font: Option<String>,
    #[serde(default)]
    deterministic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StampImageParams {
    doc: u64,
    image_base64: String,
    width: Option<f64>,
    position: Option<String>,
    opacity: Option<f32>,
    angle: Option<f64>,
    #[serde(default)]
    deterministic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCall {
    name: String,
    #[serde(default)]
    arguments: Value,
}

// ---- the methods ------------------------------------------------------------

impl Session {
    /// One plain method: the name on the wire, the params as they came.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        match method {
            "open" => self.open(params),
            "close" => self.close(params),
            "shutdown" => {
                self.stopping = true;
                Ok(Value::Null)
            }
            "info" => self.on_doc(params, |o| {
                Ok(cmd::info::report(&o.doc, Path::new(&o.name)))
            }),
            "doctor" => self.doctor(params),
            "search" => self.search(params),
            "render" => self.render(params),
            "hash" => self.on_doc(params, |o| {
                Ok(cmd::hash::report(&o.doc, Path::new(&o.name)))
            }),
            "diff" => self.diff(params),
            "forms.dump" => self.on_doc(params, |o| Ok(cmd::forms::field_rows(&o.doc))),
            "forms.fill" => self.fill(params),
            "text" => self.text(params),
            "words" => self.on_pages(params, cmd::extract::word_rows),
            "markdown" => self.on_pages(params, cmd::extract::page_markdown),
            "links" => self.on_pages(params, cmd::extract::link_rows),
            "toc" => self.on_doc(params, |o| Ok(cmd::extract::toc_rows(&o.doc))),
            "attachments" => self.on_doc(params, |o| cmd::extract::attachment_rows(&o.doc, None)),
            "annotations" => self.on_pages(params, cmd::extract::annotation_rows),
            "signatures" => self.on_doc(params, |o| Ok(cmd::extract::signature_rows(&o.doc))),
            "images" => self.images(params),
            "image" => self.image(params),
            "fonts" => self.on_doc(params, |o| cmd::extract::font_rows(&o.doc, None)),
            "object" => self.object(params),
            "xref" => self.on_doc(params, |o| {
                Ok(cmd::inspect::xref_report(&o.doc, Path::new(&o.name)))
            }),
            "revisions" => self.on_doc(params, |o| Ok(cmd::inspect::revision_rows(&o.doc))),
            "structure" => self.on_pages(params, |doc, spec| {
                cmd::inspect::structure_rows(doc, spec).map(|(rows, _)| rows)
            }),
            "pages.slice" => self.slice(params),
            "pages.merge" => self.merge(params),
            "metadata.set" => self.metadata_set(params),
            "pages.delete" => self.delete(params),
            "pages.rotate" => self.rotate(params),
            "attach.add" => self.attach_add(params),
            "attach.remove" => self.attach_remove(params),
            "stamp.text" => self.stamp_text(params),
            "stamp.image" => self.stamp_image(params),
            _ => Err(Error::method_not_found(method)),
        }
    }

    /// A method whose params are `{doc}` and whose result is one report.
    fn on_doc<T: serde::Serialize>(
        &self,
        params: Value,
        report: impl FnOnce(&Open) -> Result<T>,
    ) -> Result<Value, Error> {
        let p: DocParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        value(&failed(report(open))?)
    }

    /// A method whose params are `{doc, pages?}` and whose result is the
    /// rows of the selected pages.
    fn on_pages<T: serde::Serialize>(
        &self,
        params: Value,
        rows: impl FnOnce(&Document, Option<&str>) -> Result<T>,
    ) -> Result<Value, Error> {
        let p: PagesParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        selection(&open.doc, p.pages.as_deref())?;
        value(&failed(rows(&open.doc, p.pages.as_deref()))?)
    }

    fn open(&mut self, params: Value) -> Result<Value, Error> {
        let p: OpenParams = self::params(params)?;
        if self.docs.len() >= self.max_docs {
            return Err(Error::failed(&anyhow::anyhow!(
                "{} open document{} is the limit; close one, or start with a higher --max-docs",
                self.max_docs,
                if self.max_docs == 1 { "" } else { "s" }
            )));
        }
        let password = p.password.as_deref().or(self.password.as_deref());
        let (name, doc) = match (p.file, p.bytes_base64) {
            (Some(file), None) if out::is_stdin(Path::new(&file)) => {
                return Err(Error::invalid_params(
                    "`-` is the session's own stdin; send the file as bytes_base64",
                ));
            }
            (Some(file), None) => {
                let doc = failed(out::open(Path::new(&file), password))?;
                (file, doc)
            }
            (None, Some(text)) => {
                let bytes = unbase64(&text)
                    .ok_or_else(|| Error::invalid_params("bytes_base64 is not base64"))?;
                let doc = failed(out::open_bytes(bytes, password).context("cannot open -"))?;
                ("-".to_owned(), doc)
            }
            _ => {
                return Err(Error::invalid_params(
                    "open takes `file` or `bytes_base64`, one of the two",
                ));
            }
        };
        self.next_id += 1;
        let id = self.next_id;
        let mut report = value(&cmd::info::report(&doc, Path::new(&name)))?;
        if let Value::Object(map) = &mut report {
            map.insert("doc".to_owned(), json!(id));
        }
        self.docs.insert(id, Open { name, doc });
        Ok(report)
    }

    fn close(&mut self, params: Value) -> Result<Value, Error> {
        let p: DocParams = self::params(params)?;
        doc_of(&self.docs, p.doc)?;
        self.docs.remove(&p.doc);
        Ok(json!({"doc": p.doc}))
    }

    fn doctor(&self, params: Value) -> Result<Value, Error> {
        let p: DoctorParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        value(&cmd::doctor::report(
            &open.doc,
            Path::new(&open.name),
            p.scan_all,
        ))
    }

    fn text(&self, params: Value) -> Result<Value, Error> {
        let p: TextParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        selection(&open.doc, p.pages.as_deref())?;
        value(&failed(cmd::extract::page_texts(
            &open.doc,
            p.pages.as_deref(),
            p.layout,
        ))?)
    }

    fn search(&self, params: Value) -> Result<Value, Error> {
        let p: SearchParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        selection(&open.doc, p.pages.as_deref())?;
        let options = FindOptions {
            match_case: !p.ignore_case,
            ..FindOptions::default()
        };
        value(&failed(cmd::terminal::find(
            &open.doc,
            &p.needle,
            options,
            p.pages.as_deref(),
        ))?)
    }

    fn render(&mut self, params: Value) -> Result<Value, Error> {
        let p: RenderParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        let scale = match (p.dpi, p.scale) {
            (Some(_), Some(_)) => {
                return Err(Error::invalid_params("render takes dpi or scale, not both"));
            }
            (Some(dpi), None) => dpi / 72.0,
            (None, Some(scale)) => scale,
            (None, None) => 150.0 / 72.0,
        };
        let scale = cmd::render::checked_scale(scale)
            .map_err(|e| Error::invalid_params(out::error_line(&e)))?;
        let index = page_index(&open.doc, p.page)?;
        let pixmap = failed(cmd::render::render_page(
            &open.doc,
            index,
            scale,
            p.annotations.unwrap_or(true),
            &mut self.render,
        ))?;
        let png = failed(pixmap.encode_png().context("cannot encode PNG"))?;
        Ok(json!({
            "page": p.page,
            "width": pixmap.width(),
            "height": pixmap.height(),
            "png_base64": base64(&png),
        }))
    }

    fn images(&self, params: Value) -> Result<Value, Error> {
        let p: ImagesParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        selection(&open.doc, p.pages.as_deref())?;
        let rows: Vec<_> = failed(cmd::extract::image_rows(
            &open.doc,
            p.pages.as_deref(),
            p.all,
        ))?
        .into_iter()
        .map(|(row, _)| row)
        .collect();
        value(&rows)
    }

    fn image(&self, params: Value) -> Result<Value, Error> {
        let p: ImageParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        selection(&open.doc, p.pages.as_deref())?;
        let rows = failed(cmd::extract::image_rows(
            &open.doc,
            p.pages.as_deref(),
            p.all,
        ))?;
        let Some((_, picture)) = rows.into_iter().find(|(row, _)| row.index == p.index) else {
            return Err(Error::invalid_params(format!(
                "no image {} in that selection; `images` numbers them",
                p.index
            )));
        };
        let pixmap = picture.pixmap();
        let png = failed(pixmap.encode_png().context("cannot encode PNG"))?;
        Ok(json!({
            "index": p.index,
            "width": pixmap.width(),
            "height": pixmap.height(),
            "png_base64": base64(&png),
        }))
    }

    fn fill(&self, params: Value) -> Result<Value, Error> {
        let p: FillParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        let (bytes, set) = failed(cmd::forms::fill_values(
            &open.doc,
            &open.name,
            &p.values,
            p.deterministic,
        ))?;
        Ok(json!({"fields_set": set, "bytes_base64": base64(&bytes)}))
    }

    fn object(&self, params: Value) -> Result<Value, Error> {
        let p: ObjectParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        value(&failed(cmd::inspect::object_report(
            &open.doc,
            p.num,
            p.generation,
        ))?)
    }

    fn diff(&self, params: Value) -> Result<Value, Error> {
        let p: DiffParams = self::params(params)?;
        let left = doc_of(&self.docs, p.doc)?;
        let right = doc_of(&self.docs, p.other)?;
        let dpi = p.dpi.unwrap_or(72.0);
        cmd::render::checked_scale(dpi / 72.0)
            .map_err(|e| Error::invalid_params(out::error_line(&e)))?;
        value(&failed(cmd::diff::report(&cmd::diff::Compare {
            left: &left.doc,
            right: &right.doc,
            left_name: &left.name,
            right_name: &right.name,
            visual: p.visual,
            out_dir: None,
            dpi,
        }))?)
    }

    fn slice(&self, params: Value) -> Result<Value, Error> {
        let p: SliceParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        selection(&open.doc, p.pages.as_deref())?;
        let crop = p.crop.map(|[x0, y0, x1, y1]| Rect::new(x0, y0, x1, y1));
        let (bytes, kept) = failed(cmd::pages::slice_bytes(
            &open.doc,
            p.pages.as_deref(),
            p.rotate,
            crop,
            p.deterministic,
        ))?;
        Ok(json!({"pages": kept, "bytes_base64": base64(&bytes)}))
    }

    fn merge(&self, params: Value) -> Result<Value, Error> {
        let p: MergeParams = self::params(params)?;
        let Some((first, rest)) = p.docs.split_first() else {
            return Err(Error::invalid_params(
                "docs must name at least one open document",
            ));
        };
        let base = doc_of(&self.docs, *first)?;
        let others = rest
            .iter()
            .map(|&id| doc_of(&self.docs, id).map(|o| &o.doc))
            .collect::<Result<Vec<_>, _>>()?;
        let (bytes, count) = failed(cmd::pages::merge_bytes(&base.doc, &others, p.deterministic))?;
        Ok(json!({"pages": count, "bytes_base64": base64(&bytes)}))
    }

    // ---- the verbs that edit ------------------------------------------------

    fn metadata_set(&self, params: Value) -> Result<Value, Error> {
        let p: MetadataParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        let changes = cmd::metadata::Changes {
            title: p.title,
            author: p.author,
            subject: p.subject,
            keywords: p.keywords,
            creator: p.creator,
            clear: p.clear,
        };
        let (metadata, set, cleared) = cmd::metadata::apply(open.doc.metadata(), &changes)
            .map_err(|e| Error::invalid_params(out::error_line(&e)))?;
        let bytes = failed(cmd::metadata::set_bytes(
            &open.doc,
            &metadata,
            p.deterministic,
        ))?;
        Ok(json!({
            "keys_set": set,
            "keys_cleared": cleared,
            "bytes_base64": base64(&bytes),
        }))
    }

    fn delete(&self, params: Value) -> Result<Value, Error> {
        let p: DeleteParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        let (gone, left) = cmd::pages::deletion(&open.doc, &p.pages)
            .map_err(|e| Error::invalid_params(out::error_line(&e)))?;
        let bytes = failed(cmd::pages::delete_bytes(&open.doc, &gone, p.deterministic))?;
        Ok(json!({"deleted": gone.len(), "pages": left, "bytes_base64": base64(&bytes)}))
    }

    fn rotate(&self, params: Value) -> Result<Value, Error> {
        let p: RotateParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        selection(&open.doc, p.pages.as_deref())?;
        cmd::pages::parse_turn(p.by).map_err(|e| Error::invalid_params(out::error_line(&e)))?;
        let (bytes, turned) = failed(cmd::pages::rotate_bytes(
            &open.doc,
            p.pages.as_deref(),
            p.by,
            p.deterministic,
        ))?;
        Ok(json!({"pages": turned, "bytes_base64": base64(&bytes)}))
    }

    fn attach_add(&self, params: Value) -> Result<Value, Error> {
        let p: AttachAddParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        if p.name.trim().is_empty() {
            return Err(Error::invalid_params("name is empty"));
        }
        let bytes = unbase64(&p.bytes_base64)
            .ok_or_else(|| Error::invalid_params("bytes_base64 is not base64"))?;
        let item = cmd::attach::NewAttachment {
            name: p.name,
            bytes,
            description: p.description,
            mime: p.mime,
            modified: None,
        };
        let bytes = failed(cmd::attach::add_bytes(
            &open.doc,
            std::slice::from_ref(&item),
            p.deterministic,
        ))?;
        Ok(json!({"added": 1, "bytes_base64": base64(&bytes)}))
    }

    fn attach_remove(&self, params: Value) -> Result<Value, Error> {
        let p: AttachRemoveParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        if p.names.is_empty() {
            return Err(Error::invalid_params(
                "names must name at least one attachment",
            ));
        }
        let (bytes, removed) = failed(cmd::attach::remove_bytes(
            &open.doc,
            &p.names,
            p.deterministic,
        ))?;
        Ok(json!({"removed": removed, "bytes_base64": base64(&bytes)}))
    }

    fn stamp_text(&self, params: Value) -> Result<Value, Error> {
        let p: StampTextParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        let invalid = |e: anyhow::Error| Error::invalid_params(out::error_line(&e));
        let text = cmd::stamp::checked_text(&p.text).map_err(invalid)?;
        let type_ = cmd::stamp::Type {
            size: p.size.unwrap_or(36.0),
            color: p.color.as_deref().unwrap_or("000000"),
            font: p.font.as_deref().unwrap_or("Helvetica"),
        };
        let mark = mark(p.position.as_deref(), p.opacity, p.angle)?;
        let options = cmd::stamp::options(mark, Some(type_)).map_err(invalid)?;
        let (bytes, pages) = failed(cmd::stamp::text_bytes(
            &open.doc,
            text,
            &options,
            p.deterministic,
        ))?;
        Ok(json!({"pages": pages, "bytes_base64": base64(&bytes)}))
    }

    fn stamp_image(&self, params: Value) -> Result<Value, Error> {
        let p: StampImageParams = self::params(params)?;
        let open = doc_of(&self.docs, p.doc)?;
        let invalid = |e: anyhow::Error| Error::invalid_params(out::error_line(&e));
        let mark = mark(p.position.as_deref(), p.opacity, p.angle)?;
        let options = cmd::stamp::options(mark, None).map_err(invalid)?;
        let image = unbase64(&p.image_base64)
            .ok_or_else(|| Error::invalid_params("image_base64 is not base64"))?;
        let decoded = cmd::pages::decode_bytes(image, "image_base64").map_err(invalid)?;
        let width = cmd::stamp::checked_width(p.width, &decoded).map_err(invalid)?;
        let (bytes, pages) = failed(cmd::stamp::image_bytes(
            &open.doc,
            &decoded,
            width,
            &options,
            p.deterministic,
        ))?;
        Ok(json!({"pages": pages, "bytes_base64": base64(&bytes)}))
    }

    // ---- MCP ----------------------------------------------------------------

    /// The Model Context Protocol's methods over the same table.
    fn mcp(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": MCP_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "pdfrum", "version": env!("CARGO_PKG_VERSION")},
            })),
            // Sent without an id, so nothing goes back either way.
            "notifications/initialized" | "notifications/cancelled" => Ok(Value::Null),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => self.tool_call(params),
            "shutdown" => self.call(method, params),
            _ => Err(Error::method_not_found(method)),
        }
    }

    /// `tools/call`: the tool's method with its arguments. A wrong tool
    /// name is a protocol error; anything the method itself refuses is a
    /// result with `isError`, as the protocol has it, so the model sees
    /// the reason.
    fn tool_call(&mut self, params: Value) -> Result<Value, Error> {
        let p: ToolCall = self::params(params)?;
        let Some(method) = rpc::methods()
            .into_iter()
            .find(|m| m.tool && m.tool_name() == p.name)
        else {
            return Err(Error::invalid_params(format!(
                "no tool named {:?}; tools/list names them",
                p.name
            )));
        };
        Ok(match self.call(method.name, p.arguments) {
            Ok(result) => tool_result(result),
            Err(err) => json!({
                "content": [{"type": "text", "text": err.message}],
                "isError": true,
            }),
        })
    }
}

/// The `tools/list` entries: one per method a host may call.
fn tools() -> Vec<Value> {
    rpc::methods()
        .iter()
        .filter(|m| m.tool)
        .map(|m| {
            json!({
                "name": m.tool_name(),
                "description": m.description,
                "inputSchema": m.input_schema(),
            })
        })
        .collect()
}

/// A method's result as tool content: the JSON as text, and — when the
/// result carries a picture — the picture as an image block, taken out of
/// the text so a host does not hand a model the same base64 twice.
fn tool_result(mut result: Value) -> Value {
    let picture = result
        .as_object_mut()
        .and_then(|o| o.remove("png_base64"))
        .and_then(|v| v.as_str().map(str::to_owned));
    let mut content = vec![json!({"type": "text", "text": result.to_string()})];
    if let Some(data) = picture {
        content.push(json!({"type": "image", "data": data, "mimeType": "image/png"}));
    }
    json!({"content": content})
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdfrum::RenderSession;
    use serde_json::json;

    use super::{Session, tool_result, tools};
    use crate::rpc::{self, METHOD_NOT_FOUND};

    fn session() -> Session {
        Session {
            docs: BTreeMap::new(),
            next_id: 0,
            max_docs: 16,
            password: None,
            render: RenderSession::new(),
            stopping: false,
        }
    }

    #[test]
    fn every_method_in_the_table_is_dispatched_and_nothing_else_is() {
        let mut s = session();
        for m in rpc::methods() {
            let outcome = s.call(m.name, json!({}));
            assert!(
                !matches!(&outcome, Err(e) if e.code == METHOD_NOT_FOUND),
                "{}: in the table but not dispatched",
                m.name
            );
        }
        assert_eq!(
            s.call("nothing", json!({})).unwrap_err().code,
            METHOD_NOT_FOUND
        );
        assert_eq!(
            s.mcp("nothing", json!({})).unwrap_err().code,
            METHOD_NOT_FOUND
        );
        assert!(s.stopping, "shutdown was among the methods");
    }

    #[test]
    fn a_picture_in_a_result_becomes_an_image_block() {
        let r = tool_result(json!({"page": 1, "width": 2, "png_base64": "AAAA"}));
        assert_eq!(r["content"][0]["type"], "text");
        assert_eq!(
            r["content"][0]["text"],
            json!({"page": 1, "width": 2}).to_string()
        );
        assert_eq!(r["content"][1]["type"], "image");
        assert_eq!(r["content"][1]["data"], "AAAA");
        assert_eq!(r["content"][1]["mimeType"], "image/png");
        let plain = tool_result(json!([{"page": 1}]));
        assert_eq!(plain["content"].as_array().map(Vec::len), Some(1));
        let names: Vec<String> = tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert!(names.contains(&"forms_dump".to_owned()));
        assert!(!names.contains(&"shutdown".to_owned()));
    }
}
