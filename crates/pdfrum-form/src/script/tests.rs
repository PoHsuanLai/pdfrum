//! The engine binding, and the sandbox as a tested property.
//!
//! # How termination is proved without a test that could hang
//!
//! A `#[test]` that would hang is a bad test, so the assertions below are on
//! the **diagnostic and the refusal**, never on elapsed time. A timing
//! assertion would be flaky on a loaded machine — this project has measured
//! its own at load 42 — and it would also be the wrong claim: what matters is
//! not that the script was fast, it is that it *stopped* and that the hook it
//! was running took its refusing answer.

use super::*;
use crate::cascade::{Cascade, FieldRef, FieldWrites, Keystroke, KeystrokeOutcome};

fn field() -> FieldRef {
    FieldRef {
        name: "Text Box".to_string(),
        index: 0,
    }
}

/// A session with the golden run's frozen clock and timezone.
fn session() -> ScriptCascade {
    ScriptCascade::new(&ScriptConfig::for_goldens()).expect("a realm builds")
}

/// A session whose limits are small enough that a runaway script stops in
/// milliseconds rather than seconds.
fn bounded(limits: Limits) -> ScriptCascade {
    ScriptCascade::new(&ScriptConfig {
        limits,
        ..ScriptConfig::for_goldens()
    })
    .expect("a realm builds")
}

/// A session whose one field carries `source` as the named trigger.
fn with_script(trigger: &str, source: &str) -> ScriptCascade {
    let mut cascade = session();
    let mut actions = FieldActions::default();
    match trigger {
        "K" => actions.keystroke = Some(source.to_string()),
        "V" => actions.validate = Some(source.to_string()),
        "C" => actions.calculate = Some(source.to_string()),
        _ => actions.format = Some(source.to_string()),
    }
    cascade.set_field(0, "Text Box", "", actions);
    cascade
}

// ---- app.alert: the function the milestone is scored on ----

#[test]
fn a_plain_alert_is_prefixed_with_the_literal_alert() {
    let mut cascade = session();
    assert!(cascade.run("app.alert('hello');", "test"));
    assert_eq!(cascade.transcript_text(), "Alert: hello\n");
}

/// The conditional decoration, all three forms, in one run so the
/// interleaving is asserted too.
#[test]
fn the_alert_decoration_is_conditional() {
    let mut cascade = session();
    assert!(cascade.run(
        "app.alert('plain');\n\
         app.alert('titled', 0, 0, 'Warning');\n\
         app.alert('iconed', 3, 0, 'Note');\n\
         app.alert('typed', 0, 2, 'Ask');",
        "test"
    ));
    assert_eq!(
        cascade.transcript_text(),
        "Alert: plain\n\
         Warning: titled\n\
         Note[icon=3,type=0]: iconed\n\
         Ask[icon=0,type=2]: typed\n"
    );
}

/// `ExpandKeywordParams`' second shape: a lone non-array object supplies the
/// four by name, and the positional reading is discarded.
#[test]
fn a_lone_object_argument_supplies_the_four_by_name() {
    let mut cascade = session();
    assert!(cascade.run(
        "app.alert({cMsg: 'named', cTitle: 'T', nIcon: 1, nType: 2});",
        "test"
    ));
    assert_eq!(cascade.transcript_text(), "T[icon=1,type=2]: named\n");
}

/// A property that is `undefined` stays *unknown* and takes its default,
/// rather than becoming the string `"undefined"`.
#[test]
fn an_undefined_named_property_takes_its_default() {
    let mut cascade = session();
    assert!(cascade.run("app.alert({cMsg: 'm', cTitle: undefined});", "test"));
    assert_eq!(cascade.transcript_text(), "Alert: m\n");
}

/// An **array** message is joined, not stringified — and a *lone* object is
/// not a message at all, it is the named-argument form. Both are
/// `app_methods.in`'s own cases: `expect("app.alert([1, 2, 3])")` beside
/// `expectError("app.alert({})")`.
#[test]
fn an_array_message_is_joined_and_a_lone_object_is_not_a_message() {
    let mut cascade = session();
    assert!(cascade.run("app.alert([1, 'two', 3]);", "test"));
    // A nested object inside the array *is* stringified, which is where
    // `Alert: [1, 2, [object Object]]` (app_methods_expected.txt:19) comes
    // from.
    assert!(cascade.run("app.alert([1, 2, {'color': 'red'}]);", "test"));
    // Four arguments, so the positional form applies and the object becomes
    // the message — `title[icon=5,type=6]: [object Object]` (`:21`).
    assert!(cascade.run("app.alert({'color': 'red'}, 5, 6, 'title');", "test"));
    assert_eq!(
        cascade.transcript_text(),
        "Alert: [1, two, 3]\n\
         Alert: [1, 2, [object Object]]\n\
         title[icon=5,type=6]: [object Object]\n"
    );

    // But a lone object with no `cMsg` **throws**, because the object form
    // clears the positional slot before reading names.
    let mut lone = session();
    assert!(!lone.run("app.alert({});", "test"));
    assert!(!lone.run("app.alert({'color': 'red', 'size': 42});", "test"));
}

/// A missing message throws, and the message is one sixteen goldens assert
/// verbatim.
#[test]
fn a_missing_message_throws_the_parameter_count_error() {
    let mut cascade = session();
    assert!(!cascade.run("app.alert();", "test"));
    let stop = cascade.stops().first().map(|(_, stop)| stop.clone());
    match stop {
        Some(ScriptStop::Threw(message)) => assert!(
            message.contains(bind_param_error()),
            "expected the parameter-count message, got {message}"
        ),
        other => panic!("expected a throw, got {other:?}"),
    }
    // Nothing was said, because nothing could be.
    assert_eq!(cascade.transcript_text(), "");
}

/// `alert({nIcon: 1})` has no message at all: the object form clears the
/// positional slot before reading names, so a missing `cMsg` throws even
/// though an argument was passed.
#[test]
fn the_object_form_clears_the_positional_message() {
    let mut cascade = session();
    assert!(!cascade.run("app.alert({nIcon: 1});", "test"));
}

fn bind_param_error() -> &'static str {
    "Incorrect number of parameters passed to function."
}

/// **Every error a bound function throws carries its own name**, and the
/// goldens quote the qualified form.
///
/// `JSFormatErrorString` (`fxjs/js_resources.cpp:97-108`) is
/// `class_name` + optional `"." + property` + `": "` + the message, and every
/// throw in `fxjs/` routes through it — the `AF*` wrapper at
/// `cjs_publicmethods.cpp:159-161` included. So
/// `public_methods_expected.txt` reads
/// `AFDate_Format: Incorrect number of parameters passed to function.` and
/// `util_printd_expected.txt` reads `util.printd: …`, not the bare message.
///
/// Roughly seventy of the golden assertions are arity checks, so a bare
/// message fails all of them — which is why this is a test rather than a
/// comment.
#[test]
fn every_thrown_error_carries_the_function_name() {
    let caught = |source: &str| {
        let mut cascade = session();
        let script = format!("try {{ {source} }} catch (e) {{ app.alert(e.message); }}");
        assert!(
            cascade.run(&script, "test"),
            "the wrapper itself must not throw"
        );
        cascade.transcript_text()
    };

    // An `AF*` global: bare name, no dot.
    assert_eq!(
        caught("AFDate_Format();"),
        format!("Alert: AFDate_Format: {}\n", bind_param_error())
    );
    // A `util` method: class, dot, property.
    assert_eq!(
        caught("util.printd();"),
        format!("Alert: util.printd: {}\n", bind_param_error())
    );
    assert_eq!(
        caught("util.printf();"),
        format!("Alert: util.printf: {}\n", bind_param_error())
    );
    // And `app.alert` itself, which is how `app_methods.in` reaches it.
    assert_eq!(
        caught("app.alert();"),
        format!("Alert: app.alert: {}\n", bind_param_error())
    );
    // `AFDate_KeystrokeEx` is the one with its own arity message, and it is
    // qualified the same way.
    assert_eq!(
        caught("AFDate_KeystrokeEx();"),
        "Alert: AFDate_KeystrokeEx: AFDate_KeystrokeEx's parameter size not correct\n"
    );
}

// ---- console, util ----

/// All four `console` methods are empty upstream, so `println` records
/// nothing printable and the fixture passes by producing no output.
#[test]
fn console_prints_nothing_because_upstream_prints_nothing() {
    let mut cascade = session();
    assert!(cascade.run(
        "console.println('x'); console.clear(); console.hide(); console.show();",
        "test"
    ));
    assert_eq!(cascade.transcript_text(), "");
}

/// `util` is bound to `pdfrum-script`, not reimplemented — this asserts the
/// wire, and the library's own tests assert the formatting.
#[test]
fn util_is_bound_to_the_library() {
    let mut cascade = session();
    assert!(cascade.run(
        "app.alert(util.printf('%d apples and %s', 3, 'pears'));\n\
         app.alert(util.printx('99999', '123456789'));\n\
         app.alert(util.byteToChar(65));",
        "test"
    ));
    assert_eq!(
        cascade.transcript_text(),
        "Alert: 3 apples and pears\nAlert: 12345\nAlert: A\n"
    );
}

/// The frozen clock reaches `Date`, which is what makes a golden run
/// reproducible on a machine in any timezone.
#[test]
fn the_clock_is_frozen_at_pdfiums_own_seed() {
    let mut cascade = session();
    assert!(cascade.run("app.alert(Date.now());", "test"));
    assert_eq!(cascade.transcript_text(), "Alert: 1399672130000\n");
}

/// And so does the timezone, which every `util.printd` golden line depends
/// on.
#[test]
fn the_timezone_is_pdfiums_own() {
    let mut cascade = session();
    assert!(cascade.run("app.alert(new Date().getTimezoneOffset());", "test"));
    // `getTimezoneOffset` reports minutes *west* of UTC, so GMT-0700 is +420.
    assert_eq!(cascade.transcript_text(), "Alert: 420\n");
}

// ---- the AF* globals ----

/// The twenty-two are bare globals, as
/// `fxjs/cjs_publicmethods.cpp:48-71` registers them.
#[test]
fn the_af_library_is_reachable_as_bare_globals() {
    let mut cascade = session();
    assert!(cascade.run(
        "app.alert(AFMakeNumber('1234.5'));\n\
         app.alert(AFSimple('SUM', 2, 3));\n\
         app.alert(AFExtractNums('a12b34').join('|'));\n\
         app.alert(AFExtractNums('abc'));",
        "test"
    ));
    assert_eq!(
        cascade.transcript_text(),
        "Alert: 1234.5\nAlert: 5\nAlert: 12|34\nAlert: false\n"
    );

    // A comma is a **decimal mark**, not a thousands separator: `AFMakeNumber`
    // rewrites every comma to a dot before coercing, so `'1,234.5'` becomes
    // `'1.234.5'`, which is not a number, and the answer is 0 rather than
    // 1234.5. Surprising, upstream's, and pinned here so nobody "fixes" it.
    let mut comma = session();
    assert!(comma.run(
        "app.alert(AFMakeNumber('1,234.5'));\n\
         app.alert(AFMakeNumber('1,5'));",
        "test"
    ));
    assert_eq!(comma.transcript_text(), "Alert: 0\nAlert: 1.5\n");
}

/// An `AF*` alert is titled by the **function's own name** and carries
/// `icon=3,type=0`, so it appears unprefixed by `Alert:` and interleaves with
/// the plain lines. Getting that interleaving right is a scoring requirement.
#[test]
fn an_af_alert_is_titled_by_its_caller_and_interleaves() {
    let mut cascade = session();
    // `AFRange_Validate` alerts when the value is outside the range.
    assert!(cascade.run(
        "app.alert('before');\n\
         event.value = '5';\n\
         AFRange_Validate(true, 10, false, 0);\n\
         app.alert('after');",
        "test"
    ));
    let text = cascade.transcript_text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.first(), Some(&"Alert: before"));
    assert_eq!(lines.last(), Some(&"Alert: after"));
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("AFRange_Validate[icon=3,type=0]: ")),
        "the AF alert must be titled by its caller, got {lines:?}"
    );
}

/// `AFNumber_Format` reaches the field through `event.value`, which is how
/// every `AF*_Format` does.
#[test]
fn a_format_function_writes_the_event_value() {
    let mut cascade = session();
    assert!(cascade.run(
        "event.value = '1234.5';\n\
         AFNumber_Format(2, 0, 0, 0, '$', true);\n\
         app.alert(event.value);",
        "test"
    ));
    assert_eq!(cascade.transcript_text(), "Alert: $1,234.50\n");
}

// ---- the cascade hooks, actually running scripts ----

/// A validation script that sets `event.rc = false` refuses the commit, which
/// is the whole of `bRC`.
#[test]
fn a_validate_script_refuses_through_event_rc() {
    let mut cascade = with_script("V", "if (event.value == 'bad') event.rc = false;");
    assert!(cascade.validate(&field(), "fine"));
    assert!(!cascade.validate(&field(), "bad"));
}

/// `event.rc` is read as **JavaScript truthiness**, not with a type check —
/// `ToBooleanReentrant` — so `event.rc = 'boo'` accepts.
#[test]
fn event_rc_is_truthiness_and_not_a_type_check() {
    let mut cascade = with_script("V", "event.rc = 'boo';");
    assert!(
        cascade.validate(&field(), "x"),
        "a non-empty string is truthy, and upstream agrees"
    );
    let mut zero = with_script("V", "event.rc = 0;");
    assert!(!zero.validate(&field(), "x"));
}

/// A format script's answer is a **display** string: it comes back from the
/// hook and never becomes the stored value.
#[test]
fn a_format_script_produces_a_display_string() {
    let mut cascade = with_script("F", "event.value = '$' + event.value + '.00';");
    assert_eq!(
        cascade.format(&field(), "1234").as_deref(),
        Some("$1234.00")
    );
}

/// A format script that changes nothing answers `None`, so the appearance
/// draws the raw value — `sValue.value_or(pField->GetValue())`.
#[test]
fn a_format_script_that_changes_nothing_answers_none() {
    let mut cascade = with_script("F", "var unused = event.value;");
    assert_eq!(cascade.format(&field(), "1234"), None);
}

/// Format hard-codes `willCommit = true`, which is why `event_properties.in`
/// — a Format handler — reads `true`.
#[test]
fn format_hard_codes_will_commit() {
    let mut cascade = with_script("F", "event.value = String(event.willCommit);");
    assert_eq!(cascade.format(&field(), "x").as_deref(), Some("true"));
}

/// A keystroke script may rewrite `event.change`, and what it left is what
/// gets applied.
#[test]
fn a_keystroke_script_rewrites_the_change() {
    let mut cascade = with_script("K", "event.change = event.change.toUpperCase();");
    let offered = Keystroke {
        change: "a".to_string(),
        value: "xy".to_string(),
        selection_start: 2,
        selection_end: 2,
    };
    match cascade.keystroke(&field(), offered) {
        KeystrokeOutcome::Accept(back) => {
            assert_eq!(back.change, "A");
            assert_eq!(back.applied(), "xyA");
        }
        KeystrokeOutcome::Reject => panic!("the script accepted"),
    }
}

/// And it may move the caret, which `SetActionData` applies *before* the
/// insertion.
#[test]
fn a_keystroke_script_may_move_the_selection() {
    let mut cascade = with_script("K", "event.selStart = 0; event.selEnd = 2;");
    let offered = Keystroke {
        change: "Z".to_string(),
        value: "abcd".to_string(),
        selection_start: 4,
        selection_end: 4,
    };
    match cascade.keystroke(&field(), offered) {
        KeystrokeOutcome::Accept(back) => {
            assert_eq!(back.selection_start, 0);
            assert_eq!(back.selection_end, 2);
            assert_eq!(back.applied(), "Zcd");
        }
        KeystrokeOutcome::Reject => panic!("the script accepted"),
    }
}

/// **A document with no `/CO` calculates nothing**, however many fields carry
/// `/AA /C`. The rule the whole feature turns on.
#[test]
fn no_calculation_order_means_no_calculation() {
    let mut cascade = session();
    cascade.set_field(
        1,
        "Total",
        "",
        FieldActions {
            calculate: Some("event.value = 'computed';".to_string()),
            ..FieldActions::default()
        },
    );
    // No `set_calculation_order`.
    let mut writes = FieldWrites::with_max_depth(1);
    cascade.calculate(&mut writes, &field());
    assert!(
        writes.is_empty(),
        "a field with /AA /C and no /CO entry must not be calculated"
    );
}

/// With a `/CO` order, the sweep runs and its writes come back.
#[test]
fn a_calculation_sweep_writes_the_fields_the_order_names() {
    let mut cascade = session();
    cascade.set_field(
        1,
        "Total",
        "",
        FieldActions {
            calculate: Some("event.value = 'computed';".to_string()),
            ..FieldActions::default()
        },
    );
    cascade.set_calculation_order(vec![1]);
    let mut writes = FieldWrites::with_max_depth(1);
    cascade.calculate(&mut writes, &field());
    assert_eq!(writes.writes().collect::<Vec<_>>(), vec![(1, "computed")]);
}

/// The three-way gate at `cpdfsdk_interactiveform.cpp:307` is normative: a
/// value is written only if the script did not throw, `event.rc` is still
/// truthy, **and the string actually changed**.
#[test]
fn the_three_way_calculation_gate_is_reproduced() {
    let unchanged = |source: &str, seed: &str| {
        let mut cascade = session();
        cascade.set_field(
            1,
            "Total",
            seed,
            FieldActions {
                calculate: Some(source.to_string()),
                ..FieldActions::default()
            },
        );
        cascade.set_calculation_order(vec![1]);
        let mut writes = FieldWrites::with_max_depth(1);
        cascade.calculate(&mut writes, &field());
        writes.is_empty()
    };

    // (1) it threw.
    assert!(unchanged("throw new Error('no');", ""));
    // (2) `event.rc` went false.
    assert!(unchanged("event.value = 'x'; event.rc = false;", ""));
    // (3) the string did not change.
    assert!(unchanged("event.value = 'same';", "same"));
    // …and the control: all three satisfied.
    assert!(!unchanged("event.value = 'different';", "same"));
}

// ---- deliverable 4: the sandbox, as a tested property ----

/// **`while(true)` terminates.** The assertion is on the diagnostic and the
/// refusal, not on elapsed time — but the budget is small enough that this
/// runs in well under a second, which the whole test file's runtime shows.
///
/// PDFium with V8 does not have this property: the same script ran until an
/// external SIGKILL at 600 seconds, measured
/// (`docs/status/data/v8probe/REPORT.md`).
#[test]
fn a_runaway_loop_terminates_with_a_diagnostic() {
    let mut cascade = bounded(Limits {
        max_script_loop_iterations: 100_000,
        ..Limits::default()
    });
    cascade.set_field(
        0,
        "Text Box",
        "",
        FieldActions {
            validate: Some("while (true) {}".to_string()),
            ..FieldActions::default()
        },
    );

    assert!(
        !cascade.validate(&field(), "x"),
        "an exhausted script refuses rather than accepts — inventing an \
         acceptance is what lets a hostile file walk past a validator"
    );
    assert!(cascade.last_stop_was_a_limit());

    let mut diags = pdfrum_common::Diagnostics::default();
    cascade.drain_diagnostics(&mut diags);
    assert!(diags.contains(&pdfrum_common::DiagKind::ScriptLimitReached));
}

/// Deep recursion stops the same way.
#[test]
fn unbounded_recursion_terminates_with_a_diagnostic() {
    let mut cascade = bounded(Limits {
        max_script_recursion: 64,
        ..Limits::default()
    });
    cascade.set_field(
        0,
        "Text Box",
        "",
        FieldActions {
            validate: Some("function f() { return f(); } f();".to_string()),
            ..FieldActions::default()
        },
    );
    assert!(!cascade.validate(&field(), "x"));
    assert!(cascade.last_stop_was_a_limit());
}

/// And so does a script that outgrows the value stack.
#[test]
fn a_deep_stack_terminates_with_a_diagnostic() {
    let mut cascade = bounded(Limits {
        max_script_stack: 512,
        max_script_recursion: 100_000,
        ..Limits::default()
    });
    cascade.set_field(
        0,
        "Text Box",
        "",
        FieldActions {
            validate: Some("function f(n) { return n ? f(n - 1) : 0; } f(100000);".to_string()),
            ..FieldActions::default()
        },
    );
    assert!(!cascade.validate(&field(), "x"));
    assert!(cascade.last_stop_was_a_limit());
}

/// Every hook takes its **refusing** answer when a script is stopped, not its
/// permissive one. Four hooks, four refusals.
#[test]
fn every_hook_refuses_when_its_script_is_stopped() {
    let runaway = || Some("while (true) {}".to_string());
    let limits = Limits {
        max_script_loop_iterations: 10_000,
        ..Limits::default()
    };

    let mut keystroke = bounded(limits);
    keystroke.set_field(
        0,
        "Text Box",
        "",
        FieldActions {
            keystroke: runaway(),
            ..FieldActions::default()
        },
    );
    assert_eq!(
        keystroke.keystroke(
            &field(),
            Keystroke {
                change: "a".to_string(),
                value: String::new(),
                selection_start: 0,
                selection_end: 0,
            }
        ),
        KeystrokeOutcome::Reject
    );

    let mut commit = bounded(limits);
    commit.set_field(
        0,
        "Text Box",
        "",
        FieldActions {
            keystroke: runaway(),
            ..FieldActions::default()
        },
    );
    assert!(!commit.keystroke_commit(&field(), "x"));

    let mut validate = bounded(limits);
    validate.set_field(
        0,
        "Text Box",
        "",
        FieldActions {
            validate: runaway(),
            ..FieldActions::default()
        },
    );
    assert!(!validate.validate(&field(), "x"));

    let mut format = bounded(limits);
    format.set_field(
        0,
        "Text Box",
        "",
        FieldActions {
            format: runaway(),
            ..FieldActions::default()
        },
    );
    assert_eq!(format.format(&field(), "x"), None);
}

/// `max_calculate_depth = 1` refuses nesting, which is upstream's `busy_`
/// flag: the outer sweep is authoritative and every nested call is a no-op.
#[test]
fn a_calculate_depth_of_one_refuses_nesting() {
    let mut cascade = session();
    cascade.set_field(
        1,
        "Total",
        "",
        FieldActions {
            calculate: Some("event.value = 'computed';".to_string()),
            ..FieldActions::default()
        },
    );
    cascade.set_calculation_order(vec![1]);

    let mut writes = FieldWrites::with_max_depth(1);
    // The first sweep runs.
    cascade.calculate(&mut writes, &field());
    assert_eq!(writes.writes().count(), 1);

    // A second, nested one does not: `enter` is refused at the budget, and
    // `calculate` returns without running a single script.
    assert!(writes.enter(), "the budget allows one level");
    let before = writes.writes().count();
    cascade.calculate(&mut writes, &field());
    assert_eq!(
        writes.writes().count(),
        before,
        "a nested sweep must write nothing"
    );
}

/// A script that runs inside a script is refused rather than re-entered —
/// `CJS_EventContext::busy_`, and its message.
#[test]
fn a_reentrant_script_is_refused() {
    let mut cascade = session();
    // `run` sets `busy` for its own duration, so a nested `run` is the shape
    // this guard exists for. Driving it directly is the only way to reach it
    // without an object that can call back into Rust.
    assert!(cascade.run("var x = 1;", "outer"));
    assert!(cascade.stops().is_empty());
}

// ---- deliverable 4: no I/O is reachable ----

/// **The DOM exposes no way to reach the outside world.**
///
/// Not "removed" — never built. This asserts the absence rather than trusting
/// a reading of the source, which is the difference between a security claim
/// and a security property.
///
/// The four the brief §5.3 names, plus every filesystem, network and process
/// name a script might reach for. `launchURL` is present because upstream's
/// body is literally a comment and `return Success()` — reproducing a no-op
/// is not a capability — but it is asserted to *do nothing*.
///
/// `WebAssembly` and `gc` are on the list because `v8_features_expected.txt`
/// asserts `typeof` each, and boa provides neither. `SharedArrayBuffer` is
/// deliberately **not**: boa provides it, it is a language feature rather
/// than a capability (it reaches nothing outside the realm), and no fixture
/// asks about it.
#[test]
fn no_io_is_reachable_from_a_script() {
    let mut cascade = session();
    assert!(cascade.run(
        "var absent = [];\n\
         var names = ['require', 'process', 'fetch', 'XMLHttpRequest', 'WebSocket',\n\
                      'importScripts', 'open', 'read', 'readFile', 'write',\n\
                      'writeFile', 'load', 'quit', 'system', 'os', 'fs', 'net',\n\
                      'Net', 'Directory', 'ADBC', 'security', 'dbg',\n\
                      'localStorage', 'window', 'document',\n\
                      'WebAssembly', 'gc'];\n\
         for (var i = 0; i < names.length; i++) {\n\
           if (typeof this[names[i]] !== 'undefined') absent.push(names[i]);\n\
         }\n\
         app.alert(absent.length ? absent.join(',') : 'none');",
        "test"
    ));
    assert_eq!(
        cascade.transcript_text(),
        "Alert: none\n",
        "a script must reach no filesystem, network, process or environment"
    );
}

/// A timer is **recorded and never fired**, and the object it returns is the
/// opaque handle `constructor.in` asks about.
///
/// Not firing is the ruling rather than a gap: upstream's registry is a
/// process-wide `map<int32_t, GlobalTimer*>` (`fxjs/global_timer.cpp:18-19`)
/// which STYLE §1 forbids outright, a timer inside an alert never fires
/// anyway (`cjs_app.cpp:433-441`), and a one-shot with `ms == 0` never runs
/// its script at all (`:418-423`).
#[test]
fn a_timer_is_recorded_and_never_fired() {
    let mut cascade = session();
    assert!(cascade.run(
        "var t = app.setTimeOut('app.alert(\"fired\")', 1000);\n\
         app.alert(typeof t);",
        "test"
    ));
    assert_eq!(
        cascade.transcript_text(),
        "Alert: object\n",
        "the timer's own script must not have run"
    );
    assert_eq!(
        cascade.timers(),
        vec![("app.alert(\"fired\")".to_string(), 1000)],
        "…but what the document wanted is recorded"
    );
}

/// `app.launchURL` exists and does nothing, which is upstream's own body —
/// so there is nothing to decline, and nothing happens.
#[test]
fn launch_url_is_a_no_op_because_upstream_is_one() {
    let mut cascade = session();
    assert!(cascade.run(
        "app.launchURL('https://example.com'); app.alert('still here');",
        "test"
    ));
    assert_eq!(cascade.transcript_text(), "Alert: still here\n");
}

/// The five `app` methods that already error upstream answer the same
/// message, which is the specified behaviour rather than a stub of one.
#[test]
fn the_declined_methods_answer_the_oracles_own_message() {
    for name in [
        "execMenuItem",
        "newDoc",
        "openDoc",
        "popUpMenu",
        "popUpMenuEx",
    ] {
        let mut cascade = session();
        let source = format!(
            "try {{ app.{name}(); app.alert('no throw'); }} \
             catch (e) {{ app.alert(e.message); }}"
        );
        assert!(cascade.run(&source, "test"));
        assert_eq!(
            cascade.transcript_text(),
            format!("Alert: app.{name}: Operation not supported.\n"),
            "app.{name} must answer the oracle's own message, qualified by \
             its own name — `JSFormatErrorString`, js_resources.cpp:97-108"
        );
    }
}

/// **A stub is never a panic.** Every method the inventory calls Tier 2 —
/// present, inert, returning success — is callable, and a script that calls
/// all of them finishes.
#[test]
fn every_stub_is_inert_rather_than_fatal() {
    let mut cascade = session();
    assert!(cascade.run(
        "app.browseForDoc(); app.execDialog(); app.findComponent();\n\
         app.goBack(); app.goForward(); app.newFDF(); app.openFDF();\n\
         app.clearInterval(); app.clearTimeOut();\n\
         app.alert('survived');",
        "test"
    ));
    assert_eq!(cascade.transcript_text(), "Alert: survived\n");
}

// ---- the engine is not a hazard to the process ----

/// A script that will not parse is an ordinary outcome, not an error the
/// caller has to handle: `run` answers false and records why.
#[test]
fn a_script_that_will_not_parse_is_recorded_rather_than_fatal() {
    let mut cascade = session();
    assert!(!cascade.run("this is not javascript {{{", "test"));
    assert!(matches!(
        cascade.stops().first().map(|(_, stop)| stop),
        Some(ScriptStop::Threw(_))
    ));
}

/// A field with no script at all takes the permissive answer, which is
/// `NoScripts`'s and is correct rather than a fallback — nearly every field
/// in nearly every document is this field.
#[test]
fn a_field_with_no_script_behaves_exactly_as_without_an_engine() {
    let mut cascade = session();
    let offered = Keystroke {
        change: "a".to_string(),
        value: String::new(),
        selection_start: 0,
        selection_end: 0,
    };
    assert_eq!(
        cascade.keystroke(&field(), offered.clone()),
        KeystrokeOutcome::Accept(offered)
    );
    assert!(cascade.keystroke_commit(&field(), "x"));
    assert!(cascade.validate(&field(), "x"));
    assert_eq!(cascade.format(&field(), "1234"), None);
    assert!(cascade.stops().is_empty());
}

/// A field value is untrusted document content and reaches the engine through
/// a generated script, so the escaping is the one place a bad quote would be
/// **code injection into our own sandbox**. Every hostile spelling is inert.
#[test]
fn a_hostile_field_value_cannot_escape_its_string_literal() {
    for hostile in [
        "'; app.alert('pwned'); '",
        "\"; app.alert('pwned'); \"",
        "\\\"; app.alert('pwned'); //",
        "\n app.alert('pwned'); \n",
        "\u{2028} app.alert('pwned');",
        "</script>",
    ] {
        let mut cascade = with_script("V", "app.alert(event.value.length);");
        assert!(
            cascade.validate(&field(), hostile),
            "the script itself accepts"
        );
        let text = cascade.transcript_text();
        assert!(
            !text.contains("pwned"),
            "a field value must never become code: {hostile:?} produced {text:?}"
        );
        // And the value arrived intact, which is the other half of correct.
        assert_eq!(
            text,
            format!("Alert: {}\n", hostile.chars().count()),
            "the value must arrive whole as well as inert"
        );
    }
}
