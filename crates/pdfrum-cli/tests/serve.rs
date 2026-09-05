//! `pdfrum serve --stdio`, driven over pipes: requests in, one JSON line
//! out per request, and nothing else on stdout.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// The server run on `requests`, each on its own line, stdin closed after
/// the last: every line of stdout parsed (a line that is not JSON is the
/// error), the exit status, and stderr.
fn serve(args: &[&str], requests: &[Value]) -> std::io::Result<(Vec<Value>, i32, String)> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pdfrum"))
        .arg("serve")
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        for r in requests {
            writeln!(stdin, "{r}")?;
        }
    }
    let out = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let replies = stdout
        .lines()
        .map(|l| {
            serde_json::from_str(l)
                .map_err(|e| std::io::Error::other(format!("not JSON ({e}): {l}")))
        })
        .collect::<std::io::Result<Vec<Value>>>()?;
    Ok((
        replies,
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

fn request(id: u64, method: &str, params: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn open(id: u64, file: &str) -> Value {
    request(id, "open", &json!({"file": file}))
}

/// The reply to `id`.
fn reply(replies: &[Value], id: u64) -> Result<&Value, String> {
    let reply = replies
        .iter()
        .find(|r| r["id"] == json!(id))
        .ok_or_else(|| format!("no reply to {id}"))?;
    if reply["jsonrpc"] != "2.0" {
        return Err(format!("not a JSON-RPC 2.0 reply: {reply}"));
    }
    Ok(reply)
}

/// The `result` of the reply to `id`; an error reply is the error.
fn result(replies: &[Value], id: u64) -> Result<&Value, String> {
    let reply = reply(replies, id)?;
    if reply.get("error").is_some() {
        return Err(format!("request {id} failed: {}", reply["error"]));
    }
    Ok(&reply["result"])
}

/// The error code and message of the reply to `id`; a result is the error.
fn error(replies: &[Value], id: u64) -> Result<(i64, String), String> {
    let reply = reply(replies, id)?;
    if reply.get("result").is_some() {
        return Err(format!("request {id} succeeded: {reply}"));
    }
    match (
        reply["error"]["code"].as_i64(),
        reply["error"]["message"].as_str(),
    ) {
        (Some(code), Some(message)) => Ok((code, message.to_owned())),
        _ => Err(format!("not an error object: {reply}")),
    }
}

/// Standard base64, for sending a fixture as bytes and for comparing what
/// the server sends back with what the command line writes.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0);
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        let sextet = |shift: u32| {
            TABLE
                .get(((n >> shift) & 0x3f) as usize)
                .copied()
                .unwrap_or(b'A')
        };
        out.push(char::from(sextet(18)));
        out.push(char::from(sextet(12)));
        out.push(if chunk.len() > 1 {
            char::from(sextet(6))
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(sextet(0))
        } else {
            '='
        });
    }
    out
}

fn fixture(name: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
}

const HELLO: &str = "fixtures/hello_world_2_pages.pdf";
/// What a PNG's first bytes are in base64.
const PNG: &str = "iVBORw0KGgo";

#[test]
fn a_session_opens_once_and_answers_the_commands_with_their_shapes() {
    let (replies, code, stderr) = serve(
        &["--stdio"],
        &[
            open(1, HELLO),
            request(2, "info", &json!({"doc": 1})),
            request(3, "text", &json!({"doc": 1, "pages": "1"})),
            request(4, "words", &json!({"doc": 1, "pages": "2"})),
            request(5, "render", &json!({"doc": 1, "page": 2, "dpi": 36})),
            request(
                6,
                "search",
                &json!({"doc": 1, "needle": "WORLD", "ignore_case": true}),
            ),
            request(7, "close", &json!({"doc": 1})),
            request(8, "info", &json!({"doc": 1})),
        ],
    )
    .unwrap();
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stderr, "", "nothing on stderr without --verbose");
    assert_eq!(replies.len(), 8, "one reply per request");
    let opened = result(&replies, 1).unwrap();
    assert_eq!(opened["doc"], 1);
    assert_eq!(opened["pages"], 2);
    assert_eq!(opened["file"], HELLO);
    let info = result(&replies, 2).unwrap();
    assert_eq!(info["pages"], 2);
    assert!(info.get("doc").is_none(), "info is the command's document");
    assert_eq!(
        result(&replies, 3).unwrap(),
        &json!([{"page": 1, "text": "Hello, world!\nGoodbye, world!"}])
    );
    let words = result(&replies, 4).unwrap().as_array().unwrap();
    assert_eq!(words.len(), 4);
    assert_eq!(words[0]["page"], 2);
    assert_eq!(words[0]["text"], "Hello,");
    assert_eq!(words[0]["font"], "Times-Roman");
    assert_eq!(words[0]["size"], 12.0);
    let picture = result(&replies, 5).unwrap();
    assert_eq!(picture["page"], 2);
    assert_eq!(picture["width"], 100);
    assert_eq!(picture["height"], 100);
    assert!(picture["png_base64"].as_str().unwrap().starts_with(PNG));
    let hits = result(&replies, 6).unwrap().as_array().unwrap();
    assert_eq!(hits.len(), 4, "two lines a page, both pages");
    assert_eq!(hits[0]["line"], "Hello, world!");
    assert_eq!(hits[0]["page"], 1);
    assert!(hits[0]["rects"].is_array());
    assert_eq!(result(&replies, 7).unwrap(), &json!({"doc": 1}));
    let (code, message) = error(&replies, 8).unwrap();
    assert_eq!(code, -32602, "closed is gone");
    assert!(
        message.starts_with("pdfrum: no document is open as 1"),
        "{message}"
    );
}

#[test]
fn the_errors_carry_the_codes_and_the_cli_messages() {
    let (replies, code, _) = serve(
        &["--stdio"],
        &[
            request(1, "nothing", &json!({})),
            request(2, "info", &json!({"doc": 7})),
            open(3, "fixtures/nope.pdf"),
            json!("not a request"),
            open(4, HELLO),
            request(5, "text", &json!({"doc": 1, "pages": "9"})),
            request(6, "text", &json!({"doc": 1, "layouts": true})),
            request(
                7,
                "render",
                &json!({"doc": 1, "page": 1, "dpi": 72, "scale": 1.0}),
            ),
            request(8, "render", &json!({"doc": 1, "page": 3})),
            request(9, "open", &json!({"file": "-"})),
            request(10, "open", &json!({"bytes_base64": "not*base64"})),
            request(11, "image", &json!({"doc": 1, "index": 1})),
            json!({"id": 12, "method": "info"}),
        ],
    )
    .unwrap();
    assert_eq!(code, 0);
    let (code, message) = error(&replies, 1).unwrap();
    assert_eq!(
        (code, message.as_str()),
        (-32601, "pdfrum: no method named \"nothing\"")
    );
    assert_eq!(error(&replies, 2).unwrap().0, -32602, "an unknown doc id");
    let (code, message) = error(&replies, 3).unwrap();
    assert_eq!(code, -32000, "a command error");
    assert!(
        message.starts_with("pdfrum: cannot open fixtures/nope.pdf"),
        "{message}"
    );
    let bad = replies.iter().find(|r| r["id"].is_null()).unwrap();
    assert_eq!(
        bad["error"]["code"], -32600,
        "JSON, but not a request: {bad}"
    );
    assert_eq!(error(&replies, 5).unwrap().0, -32602, "a page past the end");
    let (code, message) = error(&replies, 6).unwrap();
    assert_eq!(code, -32602);
    assert!(message.contains("unknown field `layouts`"), "{message}");
    assert_eq!(
        error(&replies, 7).unwrap().0,
        -32602,
        "dpi and scale together"
    );
    assert_eq!(error(&replies, 8).unwrap().0, -32602, "page 3 of 2");
    assert_eq!(error(&replies, 9).unwrap().0, -32602, "`-` is the wire");
    assert_eq!(error(&replies, 10).unwrap().0, -32602, "not base64");
    assert_eq!(error(&replies, 11).unwrap().0, -32602, "no such image");
    assert!(
        replies.iter().all(|r| r["error"]["message"]
            .as_str()
            .is_none_or(|m| m.starts_with("pdfrum: "))),
        "every message is the CLI's line"
    );
}

#[test]
fn max_docs_refuses_the_one_too_many_until_one_is_closed() {
    let (replies, code, _) = serve(
        &["--stdio", "--max-docs", "1"],
        &[
            open(1, HELLO),
            open(2, "fixtures/bookmarks.pdf"),
            request(3, "close", &json!({"doc": 1})),
            open(4, "fixtures/bookmarks.pdf"),
        ],
    )
    .unwrap();
    assert_eq!(code, 0);
    assert_eq!(result(&replies, 1).unwrap()["doc"], 1);
    let (code, message) = error(&replies, 2).unwrap();
    assert_eq!(code, -32000);
    assert!(
        message.contains("1 open document is the limit"),
        "{message}"
    );
    result(&replies, 3).unwrap();
    assert_eq!(
        result(&replies, 4).unwrap()["doc"],
        2,
        "ids are never reused"
    );
}

#[test]
fn shutdown_ends_the_session_and_so_does_eof() {
    let (replies, code, _) = serve(
        &["--stdio"],
        &[
            open(1, HELLO),
            request(2, "shutdown", &Value::Null),
            request(3, "info", &json!({"doc": 1})),
        ],
    )
    .unwrap();
    assert_eq!(code, 0);
    assert_eq!(replies.len(), 2, "nothing after shutdown: {replies:?}");
    assert!(result(&replies, 2).unwrap().is_null());
    let (replies, code, _) = serve(&["--stdio"], &[]).unwrap();
    assert_eq!((replies.len(), code), (0, 0), "EOF alone is a clean exit");
    let (_, code, stderr) = serve(&["--stdio", "--verbose"], &[open(1, HELLO)]).unwrap();
    assert_eq!(code, 0);
    assert_eq!(stderr.trim(), "pdfrum: serve: open: ok");
}

#[test]
fn bytes_go_in_and_files_come_out_as_the_command_line_writes_them() {
    let hello = fixture("hello_world_2_pages.pdf").unwrap();
    let (replies, code, _) = serve(
        &["--stdio"],
        &[
            request(1, "open", &json!({"bytes_base64": base64(&hello)})),
            open(2, "fixtures/bookmarks.pdf"),
            request(
                3,
                "pages.slice",
                &json!({"doc": 1, "pages": "2", "rotate": 90, "deterministic": true}),
            ),
            request(
                4,
                "pages.merge",
                &json!({"docs": [1, 2], "deterministic": true}),
            ),
            request(5, "diff", &json!({"doc": 1, "other": 2})),
            open(6, "fixtures/text_form.pdf"),
            request(
                7,
                "forms.fill",
                &json!({"doc": 3, "values": {"Text Box": "filled by the session"}}),
            ),
            open(8, "fixtures/rotated_image.pdf"),
            request(9, "images", &json!({"doc": 4})),
            request(10, "image", &json!({"doc": 4, "index": 1})),
            request(11, "hash", &json!({"doc": 1})),
        ],
    )
    .unwrap();
    assert_eq!(code, 0);
    let opened = result(&replies, 1).unwrap();
    assert_eq!(opened["file"], "-", "bytes have no path");
    assert_eq!(opened["pages"], 2);
    // The same bytes the command line writes for the same request.
    let cli = Command::new(env!("CARGO_BIN_EXE_pdfrum"))
        .args([
            "pages",
            "slice",
            HELLO,
            "--pages",
            "2",
            "--rotate",
            "90",
            "--deterministic",
            "-o",
            "-",
        ])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"))
        .output()
        .unwrap();
    assert!(cli.status.success());
    let sliced = result(&replies, 3).unwrap();
    assert_eq!(sliced["pages"], 1);
    assert_eq!(sliced["bytes_base64"], base64(&cli.stdout));
    let merged = result(&replies, 4).unwrap();
    assert_eq!(merged["pages"], 4);
    assert!(
        merged["bytes_base64"]
            .as_str()
            .unwrap()
            .starts_with("JVBERi"),
        "%PDF"
    );
    let diff = result(&replies, 5).unwrap();
    assert_eq!(diff["left"], "-");
    assert_eq!(diff["right"], "fixtures/bookmarks.pdf");
    assert_eq!(diff["left_pages"], 2);
    let filled = result(&replies, 7).unwrap();
    assert_eq!(filled["fields_set"], 1);
    assert!(
        filled["bytes_base64"]
            .as_str()
            .unwrap()
            .starts_with("JVBERi")
    );
    let images = result(&replies, 9).unwrap().as_array().unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0]["index"], 1);
    assert!(
        images[0].get("written").is_none(),
        "the session writes no files"
    );
    let image = result(&replies, 10).unwrap();
    assert_eq!(
        (&image["width"], &image["height"]),
        (&json!(50), &json!(50))
    );
    assert!(image["png_base64"].as_str().unwrap().starts_with(PNG));
    let hash = result(&replies, 11).unwrap();
    assert_eq!(hash["file"], "-");
    assert_eq!(hash["sha256"].as_str().map(str::len), Some(64));
}

/// A written file's bytes as the command line writes them for `args`
/// with `--deterministic -o -`.
fn cli_bytes(args: &[&str]) -> std::io::Result<Vec<u8>> {
    let out = Command::new(env!("CARGO_BIN_EXE_pdfrum"))
        .args(args)
        .args(["--deterministic", "-o", "-"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"))
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    Ok(out.stdout)
}

#[test]
fn the_verbs_hand_the_file_back_as_the_command_line_writes_it() {
    let mona = fixture("mona_lisa.jpg").unwrap();
    let (replies, code, stderr) = serve(
        &["--stdio"],
        &[
            open(1, HELLO),
            request(
                2,
                "pages.rotate",
                &json!({"doc": 1, "by": 90, "deterministic": true}),
            ),
            request(
                3,
                "metadata.set",
                &json!({"doc": 1, "title": "T", "author": "A", "clear": ["keywords"], "deterministic": true}),
            ),
            request(
                4,
                "pages.delete",
                &json!({"doc": 1, "pages": "2", "deterministic": true}),
            ),
            request(
                5,
                "attach.add",
                &json!({"doc": 1, "name": "notes.txt", "bytes_base64": "UmVhZCBtZQ=="}),
            ),
            request(
                6,
                "stamp.text",
                &json!({"doc": 1, "text": "DRAFT", "position": "bottom-right", "opacity": 0.5}),
            ),
            request(
                7,
                "stamp.image",
                &json!({"doc": 1, "image_base64": base64(&mona), "width": 40}),
            ),
            open(8, "fixtures/embedded_attachments_with_desc.pdf"),
            request(9, "attach.remove", &json!({"doc": 2, "names": ["2.txt", "4.txt"]})),
        ],
    )
    .unwrap();
    assert_eq!(code, 0, "{stderr}");
    let bytes_of = |id: u64| -> Vec<u8> {
        let text = result(&replies, id).unwrap()["bytes_base64"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(text.starts_with("JVBERi"), "{id}: %PDF");
        text.into_bytes()
    };
    // The same bytes the command line writes for the same request.
    let turned = result(&replies, 2).unwrap();
    assert_eq!(turned["pages"], 2);
    assert_eq!(
        turned["bytes_base64"],
        base64(&cli_bytes(&["pages", "rotate", HELLO, "--by", "90"]).unwrap())
    );
    let set = result(&replies, 3).unwrap();
    assert_eq!(
        (&set["keys_set"], &set["keys_cleared"]),
        (&json!(2), &json!(1))
    );
    assert_eq!(
        set["bytes_base64"],
        base64(
            &cli_bytes(&[
                "metadata", "set", HELLO, "--title", "T", "--author", "A", "--clear", "keywords"
            ])
            .unwrap()
        )
    );
    let fewer = result(&replies, 4).unwrap();
    assert_eq!((&fewer["deleted"], &fewer["pages"]), (&json!(1), &json!(1)));
    assert_eq!(
        fewer["bytes_base64"],
        base64(&cli_bytes(&["pages", "delete", HELLO, "--pages", "2"]).unwrap())
    );
    assert_eq!(result(&replies, 5).unwrap()["added"], 1);
    assert_eq!(result(&replies, 6).unwrap()["pages"], 2);
    assert_eq!(result(&replies, 7).unwrap()["pages"], 2);
    assert_eq!(result(&replies, 9).unwrap()["removed"], 2);
    for id in [5, 6, 7, 9] {
        bytes_of(id);
    }
}

#[test]
fn a_wrong_verb_request_is_refused_before_any_work() {
    let (replies, code, stderr) = serve(
        &["--stdio"],
        &[
            open(1, HELLO),
            open(2, "fixtures/embedded_attachments_with_desc.pdf"),
            request(3, "pages.delete", &json!({"doc": 1, "pages": "1-end"})),
            request(4, "pages.rotate", &json!({"doc": 1, "by": 45})),
            request(5, "metadata.set", &json!({"doc": 1, "clear": ["date"]})),
            request(6, "metadata.set", &json!({"doc": 1})),
            request(
                7,
                "stamp.text",
                &json!({"doc": 1, "text": "x", "position": "middle"}),
            ),
            request(
                8,
                "stamp.text",
                &json!({"doc": 1, "text": "x", "font": "Arial"}),
            ),
            request(
                9,
                "stamp.image",
                &json!({"doc": 1, "image_base64": "UmVhZCBtZQ=="}),
            ),
            request(
                10,
                "attach.add",
                &json!({"doc": 1, "name": "x", "bytes_base64": "*"}),
            ),
            request(11, "attach.remove", &json!({"doc": 2, "names": ["9.txt"]})),
        ],
    )
    .unwrap();
    assert_eq!(code, 0, "{stderr}");
    for (id, what) in [
        (3, "every page"),
        (4, "--by takes"),
        (5, "not a metadata key"),
        (6, "nothing to change"),
        (7, "position \"middle\""),
        (8, "standard 14"),
        (9, "not a JPEG or PNG"),
        (10, "not base64"),
    ] {
        let (code, message) = error(&replies, id).unwrap();
        assert_eq!(code, -32602, "{id}: {message}");
        assert!(message.contains(what), "{id}: {message}");
    }
    let (code, message) = error(&replies, 11).unwrap();
    assert_eq!(code, -32000, "a name the document lacks: {message}");
    assert!(message.contains("no attachment named"), "{message}");
}

/// Every key at every depth of `v`.
fn keys(v: &Value, into: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, v) in map {
                into.push(k.clone());
                keys(v, into);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| keys(v, into)),
        _ => {}
    }
}

/// One request per method, numbered by its place, on the fixture that
/// exercises the most keys.
fn schema_runs() -> Vec<(&'static str, Value)> {
    vec![
        ("open", json!({"file": "fixtures/two_signatures.pdf"})),
        (
            "open",
            json!({"file": "fixtures/parser_rebuildxref_correct.pdf"}),
        ),
        (
            "open",
            json!({"file": "fixtures/annots_action_handling.pdf"}),
        ),
        ("open", json!({"file": "fixtures/bookmarks.pdf"})),
        (
            "open",
            json!({"file": "fixtures/embedded_attachments_with_desc.pdf"}),
        ),
        ("open", json!({"file": "fixtures/annotiter.pdf"})),
        ("open", json!({"file": "fixtures/rotated_image.pdf"})),
        ("open", json!({"file": "fixtures/bigtable_mini.pdf"})),
        ("open", json!({"file": "fixtures/text_form.pdf"})),
        ("open", json!({"file": "fixtures/bug_1484283.pdf"})),
        ("open", json!({"file": "fixtures/tagged_alt_text.pdf"})),
        ("open", json!({"file": HELLO})),
        ("info", json!({"doc": 1})),
        ("doctor", json!({"doc": 2, "scan_all": true})),
        ("search", json!({"doc": 12, "needle": "world"})),
        ("render", json!({"doc": 12, "page": 1, "scale": 0.1})),
        ("hash", json!({"doc": 12})),
        (
            "diff",
            json!({"doc": 12, "other": 4, "visual": true, "dpi": 10}),
        ),
        ("forms.dump", json!({"doc": 9})),
        ("text", json!({"doc": 12, "layout": true})),
        ("words", json!({"doc": 12})),
        ("markdown", json!({"doc": 12})),
        ("links", json!({"doc": 3})),
        ("toc", json!({"doc": 4})),
        ("attachments", json!({"doc": 5})),
        ("annotations", json!({"doc": 6})),
        ("signatures", json!({"doc": 1})),
        ("images", json!({"doc": 7, "all": true})),
        ("image", json!({"doc": 7, "index": 1})),
        ("fonts", json!({"doc": 8})),
        ("object", json!({"doc": 12, "num": 1, "gen": 0})),
        ("xref", json!({"doc": 10})),
        ("revisions", json!({"doc": 10})),
        ("structure", json!({"doc": 11})),
        (
            "forms.fill",
            json!({"doc": 9, "values": {"Text Box": "x"}, "deterministic": true}),
        ),
        (
            "pages.slice",
            json!({"doc": 12, "pages": "1", "crop": [0, 0, 100, 100]}),
        ),
        ("pages.merge", json!({"docs": [12, 4]})),
        (
            "metadata.set",
            json!({"doc": 12, "title": "T", "clear": ["author"], "deterministic": true}),
        ),
        ("pages.delete", json!({"doc": 12, "pages": "1"})),
        ("pages.rotate", json!({"doc": 12, "pages": "1", "by": 90})),
        (
            "attach.add",
            json!({"doc": 12, "name": "notes.txt", "bytes_base64": "UmVhZCBtZQ==", "description": "d", "mime": "text/plain"}),
        ),
        ("attach.remove", json!({"doc": 5, "names": ["1.txt"]})),
        (
            "stamp.text",
            json!({"doc": 12, "text": "DRAFT", "position": "top-left", "opacity": 0.5, "angle": 30, "size": 12, "color": "ff0000", "font": "Courier"}),
        ),
        (
            "stamp.image",
            json!({"doc": 12, "image_base64": base64(&fixture("mona_lisa.jpg").unwrap_or_default()), "width": 50, "position": "bottom-right"}),
        ),
        ("close", json!({"doc": 12})),
    ]
}

#[test]
fn every_result_has_only_the_keys_its_schema_shows() {
    let listing = Command::new(env!("CARGO_BIN_EXE_pdfrum"))
        .args(["schema", "serve"])
        .output()
        .unwrap();
    let table: Vec<Value> = serde_json::from_slice(&listing.stdout).unwrap();
    let example = |method: &str| -> &Value {
        &table
            .iter()
            .find(|m| m["method"] == method)
            .unwrap_or_else(|| panic!("{method} is not in `schema serve`"))["result"]
    };
    let runs = schema_runs();
    let requests: Vec<Value> = (1u64..)
        .zip(&runs)
        .map(|(i, (m, p))| request(i, m, p))
        .collect();
    let (replies, code, stderr) = serve(&["--stdio"], &requests).unwrap();
    assert_eq!(code, 0, "{stderr}");
    let mut covered = Vec::new();
    for (id, (method, _)) in (1u64..).zip(&runs) {
        let got = result(&replies, id).unwrap();
        let shape = example(method);
        assert_eq!(got.is_array(), shape.is_array(), "{method}");
        let mut expected = Vec::new();
        keys(shape, &mut expected);
        let mut real = Vec::new();
        keys(got, &mut real);
        for key in real {
            assert!(
                expected.contains(&key),
                "{method}: `{key}` is not in the schema"
            );
        }
        covered.push(*method);
    }
    for m in &table {
        let method = m["method"].as_str().unwrap();
        assert!(
            method == "shutdown" || covered.contains(&method),
            "{method} is in the table but not run here"
        );
        assert_eq!(m["tool"].is_string(), method != "shutdown");
        assert_eq!(m["params"]["type"], "object");
    }
}

#[test]
fn the_verbs_are_tools_too() {
    let (replies, code, stderr) = serve(
        &["--stdio", "--mcp"],
        &[request(1, "tools/list", &json!({}))],
    )
    .unwrap();
    assert_eq!(code, 0, "{stderr}");
    let tools = result(&replies, 1).unwrap()["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for verb in [
        "metadata_set",
        "pages_delete",
        "pages_rotate",
        "attach_add",
        "attach_remove",
        "stamp_text",
        "stamp_image",
    ] {
        assert!(names.contains(&verb), "{verb} is not a tool: {names:?}");
    }
}

#[test]
fn the_mcp_surface_lists_the_tools_and_calls_them() {
    let (replies, code, stderr) = serve(
        &["--stdio", "--mcp"],
        &[
            request(
                1,
                "initialize",
                &json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
            ),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            request(2, "tools/list", &json!({})),
            request(
                3,
                "tools/call",
                &json!({"name": "open", "arguments": {"file": HELLO}}),
            ),
            request(
                4,
                "tools/call",
                &json!({"name": "info", "arguments": {"doc": 1}}),
            ),
            request(
                5,
                "tools/call",
                &json!({"name": "render", "arguments": {"doc": 1, "page": 1, "dpi": 18}}),
            ),
            request(
                6,
                "tools/call",
                &json!({"name": "info", "arguments": {"doc": 9}}),
            ),
            request(
                7,
                "tools/call",
                &json!({"name": "forms.dump", "arguments": {"doc": 1}}),
            ),
            request(8, "ping", &json!({})),
            request(9, "info", &json!({"doc": 1})),
        ],
    ).unwrap();
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(replies.len(), 9, "the notification gets no reply");
    let init = result(&replies, 1).unwrap();
    assert_eq!(init["protocolVersion"], "2024-11-05");
    assert_eq!(init["capabilities"], json!({"tools": {}}));
    assert_eq!(init["serverInfo"]["name"], "pdfrum");
    assert_eq!(init["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    let tools = result(&replies, 2).unwrap()["tools"].as_array().unwrap();
    assert!(tools.len() > 20);
    for t in tools {
        let name = t["name"].as_str().unwrap();
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "{name}"
        );
        assert!(
            t["description"].as_str().is_some_and(|d| !d.is_empty()),
            "{name}"
        );
        assert_eq!(t["inputSchema"]["type"], "object", "{name}");
        assert!(t["inputSchema"]["properties"].is_object(), "{name}");
        assert!(t["inputSchema"]["required"].is_array(), "{name}");
    }
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"open") && names.contains(&"forms_dump") && names.contains(&"pages_slice")
    );
    assert!(!names.contains(&"shutdown"));
    let opened = result(&replies, 3).unwrap();
    assert_eq!(opened["content"][0]["type"], "text");
    let text: Value = serde_json::from_str(opened["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["doc"], 1);
    let info = result(&replies, 4).unwrap();
    assert_eq!(info.get("isError"), None);
    let text: Value = serde_json::from_str(info["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["pages"], 2);
    let picture = result(&replies, 5).unwrap()["content"].as_array().unwrap();
    assert_eq!(picture.len(), 2);
    let text: Value = serde_json::from_str(picture[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, json!({"page": 1, "width": 50, "height": 50}));
    assert_eq!(picture[1]["type"], "image");
    assert_eq!(picture[1]["mimeType"], "image/png");
    assert!(picture[1]["data"].as_str().unwrap().starts_with(PNG));
    let failed = result(&replies, 6).unwrap();
    assert_eq!(failed["isError"], true);
    assert!(
        failed["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("pdfrum: no document is open as 9")
    );
    let (code, message) = error(&replies, 7).unwrap();
    assert_eq!(code, -32602, "the tool is forms_dump: {message}");
    assert_eq!(result(&replies, 8).unwrap(), &json!({}));
    assert_eq!(
        error(&replies, 9).unwrap().0,
        -32601,
        "a plain method is not on the MCP surface"
    );
}
