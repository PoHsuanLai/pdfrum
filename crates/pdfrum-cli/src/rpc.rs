//! JSON-RPC 2.0 over lines: the wire `pdfrum serve` speaks, and the table
//! of methods it answers to.
//!
//! One request per line in, one response per line out, no framing but the
//! newline. The methods mirror the commands and their results are the
//! commands' `--json` documents — the examples `pdfrum schema` prints —
//! so a client learns one set of shapes. The table here is what
//! `tools/list` and `pdfrum schema serve` both read, and a unit test in
//! `cmd::serve` keeps it and the dispatch in step.

use std::fmt::Display;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{out, schema};

/// The line was not JSON.
pub const PARSE_ERROR: i64 = -32700;
/// JSON, but not a request: no `method`, or a `jsonrpc` that is not `2.0`.
pub const INVALID_REQUEST: i64 = -32600;
/// No such method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// The params were wrong: a missing or mistyped field, an unknown document
/// id, a page that is not in the document.
pub const INVALID_PARAMS: i64 = -32602;
/// The command ran and failed; the message is what the CLI would print.
pub const COMMAND_FAILED: i64 = -32000;

/// A request as it came off the wire.
#[derive(Deserialize)]
struct Wire {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

/// One request: `id` is `None` for a notification, which gets no reply.
#[derive(Debug)]
pub struct Request {
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
}

/// A JSON-RPC error: the code, and the `pdfrum: …` line the CLI would
/// have printed for the same mistake.
#[derive(Debug)]
pub struct Error {
    pub code: i64,
    pub message: String,
}

impl Error {
    fn new(code: i64, what: impl Display) -> Self {
        Self {
            code,
            message: format!("pdfrum: {what}"),
        }
    }

    /// The params were wrong, as they came.
    pub fn invalid_params(what: impl Display) -> Self {
        Self::new(INVALID_PARAMS, what)
    }

    /// The command failed: the CLI's error line, with the same causes.
    pub fn failed(err: &anyhow::Error) -> Self {
        Self::new(COMMAND_FAILED, out::error_line(err))
    }

    pub fn method_not_found(name: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("no method named {name:?}"))
    }
}

/// A line into a request: -32700 when it is not JSON, -32600 when it is
/// JSON but not a JSON-RPC 2.0 request.
pub fn parse(line: &str) -> Result<Request, Error> {
    let value: Value = serde_json::from_str(line)
        .map_err(|e| Error::new(PARSE_ERROR, format!("not JSON: {e}")))?;
    let wire: Wire = serde_json::from_value(value)
        .map_err(|e| Error::new(INVALID_REQUEST, format!("not a request: {e}")))?;
    if wire.jsonrpc.as_deref() != Some("2.0") {
        return Err(Error::new(
            INVALID_REQUEST,
            "not a request: `jsonrpc` must be \"2.0\"",
        ));
    }
    Ok(Request {
        id: wire.id,
        method: wire.method,
        params: wire.params,
    })
}

/// The reply to `id` with `result`.
pub fn response(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// The reply to `id` with `err`.
pub fn failure(id: &Value, err: &Error) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": err.code, "message": err.message}})
}

// ---- the method table ------------------------------------------------------

/// One parameter of a method: its name, its JSON Schema, and whether a
/// request must carry it.
pub struct Param {
    pub name: &'static str,
    pub schema: Value,
    pub required: bool,
}

/// One method the session answers to.
pub struct Method {
    /// The name on the wire: the command's words with a dot, `forms.dump`.
    pub name: &'static str,
    /// One line.
    pub description: &'static str,
    pub params: Vec<Param>,
    /// An example of the result: the command's `pdfrum schema` document,
    /// or the session's own shape for a method no command has.
    pub result: Value,
    /// Whether `tools/list` offers it; `shutdown` is not for a host to call.
    pub tool: bool,
}

impl Method {
    /// The name as an MCP tool: hosts allow `[A-Za-z0-9_-]` and nothing
    /// else, so the dot in `forms.dump` becomes an underscore.
    pub fn tool_name(&self) -> String {
        self.name.replace('.', "_")
    }

    /// The JSON Schema of the params object.
    pub fn input_schema(&self) -> Value {
        let properties: Map<String, Value> = self
            .params
            .iter()
            .map(|p| (p.name.to_owned(), p.schema.clone()))
            .collect();
        let required: Vec<&str> = self
            .params
            .iter()
            .filter(|p| p.required)
            .map(|p| p.name)
            .collect();
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }
}

fn param(name: &'static str, schema: Value, required: bool) -> Param {
    Param {
        name,
        schema,
        required,
    }
}

fn typed(kind: &str, description: &str) -> Value {
    json!({"type": kind, "description": description})
}

/// The `doc` every method about a document takes.
fn doc() -> Param {
    param("doc", typed("integer", "the id `open` returned"), true)
}

/// The `pages` selection, the `--pages` grammar.
fn pages() -> Param {
    param(
        "pages",
        typed(
            "string",
            "pages, 1-based: `3`, `1-5`, `2,7,10-end`; all by default",
        ),
        false,
    )
}

fn deterministic() -> Param {
    param(
        "deterministic",
        typed(
            "boolean",
            "reproducible bytes: the same input gives the same file",
        ),
        false,
    )
}

fn method(
    name: &'static str,
    description: &'static str,
    params: Vec<Param>,
    result: Value,
) -> Method {
    Method {
        name,
        description,
        params,
        result,
        tool: true,
    }
}

/// `value` with every `key` removed, at any depth: the `written` a
/// command puts on a row when it wrote a file, which the session never
/// does.
fn without(mut value: Value, key: &str) -> Value {
    fn strip(value: &mut Value, key: &str) {
        match value {
            Value::Object(map) => {
                map.remove(key);
                for v in map.values_mut() {
                    strip(v, key);
                }
            }
            Value::Array(items) => {
                for v in items {
                    strip(v, key);
                }
            }
            _ => {}
        }
    }
    strip(&mut value, key);
    value
}

/// The example of a command's `--json` document.
fn command(name: &str) -> Value {
    schema::example(name)
}

/// A base64 field's example.
const BASE64: &str = "JVBERi0xLjcKJeLjz9MK…";

/// Every method, in the order a session uses them: open, ask, write, close.
pub fn methods() -> Vec<Method> {
    let mut all = session();
    all.extend(document());
    all.extend(render_and_diff());
    all.extend(extract());
    all.extend(inspect());
    all.extend(writers());
    all.extend(edits());
    all.extend(attachments());
    all.extend(stamps());
    all
}

/// Opening and closing.
fn session() -> Vec<Method> {
    let mut opened = command("info");
    if let Value::Object(map) = &mut opened {
        map.insert("doc".to_owned(), json!(1));
    }
    vec![
        method(
            "open",
            "parse a document once and keep it under an id; the info report comes back with it",
            vec![
                param("file", typed("string", "the path of a PDF file"), false),
                param(
                    "bytes_base64",
                    typed("string", "the file's bytes in base64, instead of a path"),
                    false,
                ),
                param(
                    "password",
                    typed("string", "for an encrypted document"),
                    false,
                ),
            ],
            opened,
        ),
        method(
            "close",
            "forget an open document",
            vec![doc()],
            json!({"doc": 1}),
        ),
        Method {
            name: "shutdown",
            description: "end the session; closing stdin does the same",
            params: Vec::new(),
            result: Value::Null,
            tool: false,
        },
    ]
}

/// The commands that answer about a document.
fn document() -> Vec<Method> {
    vec![
        method(
            "info",
            "pages, metadata, security, signatures, identity",
            vec![doc()],
            command("info"),
        ),
        method(
            "doctor",
            "what the parser had to recover or drop",
            vec![
                doc(),
                param(
                    "scan_all",
                    typed(
                        "boolean",
                        "also build every page, so what only a page load would find is found now",
                    ),
                    false,
                ),
            ],
            command("doctor"),
        ),
        method(
            "search",
            "every hit of a text with its page, line and boxes",
            vec![
                doc(),
                param("needle", typed("string", "what to look for"), true),
                param(
                    "ignore_case",
                    typed("boolean", "match regardless of case"),
                    false,
                ),
                pages(),
            ],
            command("search"),
        ),
        method(
            "forms.dump",
            "the form fields with their kinds and values",
            vec![doc()],
            command("forms dump"),
        ),
    ]
}

/// The two that draw: a page as a picture, and two documents compared.
fn render_and_diff() -> Vec<Method> {
    vec![
        method(
            "render",
            "one page as a PNG, at a resolution or a scale",
            vec![
                doc(),
                param("page", typed("integer", "the page, 1-based"), true),
                param(
                    "dpi",
                    typed("number", "dots per inch; 150 by default"),
                    false,
                ),
                param(
                    "scale",
                    typed("number", "pixels per point, instead of dpi"),
                    false,
                ),
                param(
                    "annotations",
                    typed(
                        "boolean",
                        "draw annotations and form fields; true by default",
                    ),
                    false,
                ),
            ],
            json!({"page": 1, "width": 1275, "height": 1650, "png_base64": "iVBORw0KGgo…"}),
        ),
        method(
            "hash",
            "SHA-256 of the file, the trailer's /ID, and a semantic hash",
            vec![doc()],
            command("hash"),
        ),
        method(
            "diff",
            "what changed between two open documents: text per page, and the pixels with visual",
            vec![
                doc(),
                param(
                    "other",
                    typed("integer", "the id of the newer document"),
                    true,
                ),
                param(
                    "visual",
                    typed(
                        "boolean",
                        "render every page of both and compare the pixels too",
                    ),
                    false,
                ),
                param(
                    "dpi",
                    typed("number", "resolution for visual; 72 by default"),
                    false,
                ),
            ],
            without(command("diff"), "written"),
        ),
    ]
}

/// `extract …`.
fn extract() -> Vec<Method> {
    vec![
        method(
            "text",
            "the text of each page, in reading order",
            vec![
                doc(),
                pages(),
                param(
                    "layout",
                    typed("boolean", "keep the layout: columns stay columns"),
                    false,
                ),
            ],
            command("extract text"),
        ),
        method(
            "words",
            "every word with its box, font, size and character range",
            vec![doc(), pages()],
            command("extract words"),
        ),
        method(
            "markdown",
            "each page as Markdown",
            vec![doc(), pages()],
            command("extract markdown"),
        ),
        method(
            "links",
            "link annotations and URLs in the text",
            vec![doc(), pages()],
            command("extract links"),
        ),
        method(
            "toc",
            "the outline, one row per bookmark",
            vec![doc()],
            command("extract toc"),
        ),
        method(
            "attachments",
            "the embedded files: name, size, description",
            vec![doc()],
            without(command("extract attachments"), "written"),
        ),
        method(
            "annotations",
            "every annotation with its kind, box and text",
            vec![doc(), pages()],
            command("extract annotations"),
        ),
        method(
            "signatures",
            "the signature fields",
            vec![doc()],
            command("extract signatures"),
        ),
        method(
            "images",
            "one row per picture, with its size and format",
            vec![
                doc(),
                pages(),
                param(
                    "all",
                    typed("boolean", "every draw, repeats and spacers included"),
                    false,
                ),
            ],
            without(command("extract images"), "written"),
        ),
        method(
            "image",
            "the pixels of one picture as a PNG, by the index `images` gave it for the same selection",
            vec![
                doc(),
                param(
                    "index",
                    typed("integer", "the picture's index in `images`"),
                    true,
                ),
                pages(),
                param("all", typed("boolean", "as passed to `images`"), false),
            ],
            json!({"index": 1, "width": 50, "height": 50, "png_base64": "iVBORw0KGgo…"}),
        ),
        method(
            "fonts",
            "the embedded font programs: name, kind, object, size",
            vec![doc()],
            without(command("extract fonts"), "written"),
        ),
    ]
}

/// `inspect …`.
fn inspect() -> Vec<Method> {
    vec![
        method(
            "object",
            "one object as JSON; `pdfrum inspect object --help` gives the encoding",
            vec![
                doc(),
                param("num", typed("integer", "the object number"), true),
                param(
                    "gen",
                    typed("integer", "the generation number; 0 by default"),
                    false,
                ),
            ],
            command("inspect object"),
        ),
        method(
            "xref",
            "the cross-reference table with its trailer",
            vec![doc()],
            command("inspect xref"),
        ),
        method(
            "revisions",
            "the incremental-update history",
            vec![doc()],
            command("inspect revisions"),
        ),
        method(
            "structure",
            "the structure tree, one row per element",
            vec![doc(), pages()],
            command("inspect structure"),
        ),
    ]
}

/// The methods that make a new file; the bytes come back, nothing is
/// written on the server's behalf.
fn writers() -> Vec<Method> {
    vec![
        method(
            "forms.fill",
            "set field values and get the filled file back",
            vec![
                doc(),
                param(
                    "values",
                    json!({
                        "type": "object",
                        "description": "field name to value: a string sets the text, a boolean checks or clears, null clears",
                        "additionalProperties": true,
                    }),
                    true,
                ),
                deterministic(),
            ],
            json!({"fields_set": 2, "bytes_base64": BASE64}),
        ),
        method(
            "pages.slice",
            "keep some pages, in document order, rotated or cropped, as a new file",
            vec![
                doc(),
                pages(),
                param(
                    "rotate",
                    typed(
                        "integer",
                        "degrees to rotate the kept pages, a multiple of 90",
                    ),
                    false,
                ),
                param(
                    "crop",
                    json!({
                        "type": "array",
                        "description": "the kept pages' crop box, x0 y0 x1 y1 in points",
                        "items": {"type": "number"},
                        "minItems": 4,
                        "maxItems": 4,
                    }),
                    false,
                ),
                deterministic(),
            ],
            json!({"pages": 2, "bytes_base64": BASE64}),
        ),
        method(
            "pages.merge",
            "join open documents into one file, in the order given",
            vec![
                param(
                    "docs",
                    json!({
                        "type": "array",
                        "description": "the ids of the documents to join, first to last",
                        "items": {"type": "integer"},
                        "minItems": 1,
                    }),
                    true,
                ),
                deterministic(),
            ],
            json!({"pages": 5, "bytes_base64": BASE64}),
        ),
    ]
}

/// The mark a stamp shares between text and picture: where and how.
fn mark() -> Vec<Param> {
    vec![
        param(
            "position",
            json!({
                "type": "string",
                "description": "where on the page as displayed; center by default",
                "enum": ["center", "top-left", "top-right", "bottom-left", "bottom-right"],
            }),
            false,
        ),
        param(
            "opacity",
            typed("number", "0 (invisible) to 1 (opaque, the default)"),
            false,
        ),
        param(
            "angle",
            typed(
                "number",
                "degrees counter-clockwise about the stamp's centre; 0 by default",
            ),
            false,
        ),
    ]
}

/// A string param.
fn text(what: &str) -> Value {
    typed("string", what)
}

/// The verbs that edit the document's metadata and pages.
fn edits() -> Vec<Method> {
    vec![
        method(
            "metadata.set",
            "set or clear /Info keys and get the file back; a key not named is kept",
            vec![
                doc(),
                param("title", text("the title; empty removes the key"), false),
                param("author", text("the author"), false),
                param("subject", text("the subject"), false),
                param("keywords", text("the keywords, as one string"), false),
                param(
                    "creator",
                    text("the application the content came from"),
                    false,
                ),
                param(
                    "clear",
                    json!({
                        "type": "array",
                        "description": "keys to remove: title, author, subject, keywords, creator, producer, created, modified",
                        "items": {"type": "string"},
                    }),
                    false,
                ),
                deterministic(),
            ],
            json!({"keys_set": 2, "keys_cleared": 1, "bytes_base64": BASE64}),
        ),
        method(
            "pages.delete",
            "drop some pages and get the file back; the rest keep their order",
            vec![
                doc(),
                param(
                    "pages",
                    text("pages to delete, 1-based: `3`, `1-5`, `2,7,10-end`"),
                    true,
                ),
                deterministic(),
            ],
            json!({"deleted": 2, "pages": 9, "bytes_base64": BASE64}),
        ),
        method(
            "pages.rotate",
            "turn pages from where each one stands and get the file back",
            vec![
                doc(),
                pages(),
                param(
                    "by",
                    json!({
                        "type": "integer",
                        "description": "degrees clockwise, added to the page's own rotation",
                        "enum": [90, 180, 270, -90],
                    }),
                    true,
                ),
                deterministic(),
            ],
            json!({"pages": 3, "bytes_base64": BASE64}),
        ),
    ]
}

/// The verbs that put attachments in and take them out.
fn attachments() -> Vec<Method> {
    vec![
        method(
            "attach.add",
            "attach a file and get the document back",
            vec![
                doc(),
                param("name", text("the attachment's name"), true),
                param("bytes_base64", text("the file's bytes in base64"), true),
                param(
                    "description",
                    text("the text a viewer shows beside the name"),
                    false,
                ),
                param(
                    "mime",
                    text("the MIME type; guessed from the name's extension by default"),
                    false,
                ),
                deterministic(),
            ],
            json!({"added": 1, "bytes_base64": BASE64}),
        ),
        method(
            "attach.remove",
            "take attachments out by name and get the document back",
            vec![
                doc(),
                param(
                    "names",
                    json!({
                        "type": "array",
                        "description": "the names, as `attachments` lists them",
                        "items": {"type": "string"},
                        "minItems": 1,
                    }),
                    true,
                ),
                deterministic(),
            ],
            json!({"removed": 1, "bytes_base64": BASE64}),
        ),
    ]
}

/// The two stamps.
fn stamps() -> Vec<Method> {
    vec![
        method(
            "stamp.text",
            "draw text over every page and get the file back",
            [
                vec![doc(), param("text", text("the text"), true)],
                mark(),
                vec![
                    param(
                        "size",
                        typed("number", "the text size in points; 36 by default"),
                        false,
                    ),
                    param(
                        "color",
                        text("the text colour as RRGGBB; black by default"),
                        false,
                    ),
                    param(
                        "font",
                        text("one of the standard 14 by name; Helvetica by default"),
                        false,
                    ),
                    deterministic(),
                ],
            ]
            .into_iter()
            .flatten()
            .collect(),
            json!({"pages": 11, "bytes_base64": BASE64}),
        ),
        method(
            "stamp.image",
            "draw a JPEG or PNG over every page and get the file back",
            [
                vec![
                    doc(),
                    param(
                        "image_base64",
                        text("the picture's bytes in base64, JPEG or PNG"),
                        true,
                    ),
                    param(
                        "width",
                        typed(
                            "number",
                            "width in points, the aspect kept; the pixel width by default",
                        ),
                        false,
                    ),
                ],
                mark(),
                vec![deterministic()],
            ]
            .into_iter()
            .flatten()
            .collect(),
            json!({"pages": 11, "bytes_base64": BASE64}),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{INVALID_REQUEST, PARSE_ERROR, failure, methods, parse, response, without};

    #[test]
    fn a_line_is_a_request_or_one_of_two_errors() {
        let req = parse(r#"{"jsonrpc":"2.0","id":1,"method":"info","params":{"doc":1}}"#).unwrap();
        assert_eq!(req.id, Some(json!(1)));
        assert_eq!(req.method, "info");
        assert_eq!(req.params, json!({"doc": 1}));
        let note = parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert!(note.id.is_none(), "no id is a notification");
        assert!(note.params.is_null());
        assert_eq!(parse("{not json").unwrap_err().code, PARSE_ERROR);
        assert_eq!(parse(r#"{"id":1}"#).unwrap_err().code, INVALID_REQUEST);
        assert_eq!(
            parse(r#"{"jsonrpc":"1.0","id":1,"method":"info"}"#)
                .unwrap_err()
                .code,
            INVALID_REQUEST
        );
        assert_eq!(
            response(&json!(7), &json!({"pages": 2})),
            json!({"jsonrpc": "2.0", "id": 7, "result": {"pages": 2}})
        );
        let err = parse("nope").unwrap_err();
        assert_eq!(
            failure(&Value::Null, &err)["error"]["code"],
            json!(PARSE_ERROR)
        );
        assert!(err.message.starts_with("pdfrum: "));
    }

    #[test]
    fn every_method_has_a_result_example_and_a_tool_name_a_host_accepts() {
        let all = methods();
        assert!(all.len() > 20);
        for m in &all {
            assert!(
                m.name == "shutdown" || !m.result.is_null(),
                "{}: no result example",
                m.name
            );
            let tool = m.tool_name();
            assert!(
                tool.len() <= 64
                    && tool
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{tool}: not a tool name"
            );
            let schema = m.input_schema();
            assert_eq!(schema["type"], "object");
            for p in &m.params {
                assert!(
                    schema["properties"][p.name].is_object(),
                    "{}: {}",
                    m.name,
                    p.name
                );
            }
        }
        let names: Vec<&str> = all.iter().map(|m| m.name).collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(names.len(), unique.len(), "a method is listed twice");
        assert_eq!(
            without(
                json!([{"a": 1, "written": "x", "b": {"written": 2}}]),
                "written"
            ),
            json!([{"a": 1, "b": {}}])
        );
    }
}
