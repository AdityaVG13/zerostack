use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value as JsonValue, json};
use zero_abi::{CapabilityDescriptor, GlobalRegistration};
use zero_codemode::{
    Connector, ConnectorCompletion, ConnectorError, DispatchContext, Host, HostError, HostLimits,
};

/// No plan in this file dispatches a capability; the connector exists to
/// satisfy the host seam and fails loudly if anything tries.
struct UnusedConnector;

impl Connector for UnusedConnector {
    fn dispatch(
        &self,
        _capability: &CapabilityDescriptor,
        _args_json: &str,
        _context: DispatchContext,
        _completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        Err(ConnectorError::new("syntax test dispatched a capability"))
    }
}

fn run(plan: &str) -> JsonValue {
    let limits = HostLimits::new(
        8 * 1024 * 1024,
        256 * 1024,
        Duration::from_secs(5),
        50_000,
        1,
        2,
        4,
        4 * 1024,
        1024 * 1024,
    )
    .unwrap();
    let registration = GlobalRegistration {
        root: "z".into(),
        capabilities: vec![CapabilityDescriptor::new("z", "read")],
    };
    Host::new_zero_kernel(limits, registration)
        .unwrap()
        .execute(plan, Rc::new(UnusedConnector))
        .unwrap()
}

#[test]
fn typeof_missing_identifier_returns_undefined() {
    assert_eq!(
        run("return [typeof Buffer, typeof structuredClone];"),
        json!(["undefined", "undefined"]),
    );
}

#[test]
fn line_comments_are_ignored() {
    assert_eq!(
        run(r#"
            // leading comment
            const x = 1; // trailing comment
            let y = 2;
            // standalone comment between statements
            y = y + x;
            return y;
            "#,),
        json!(3),
    );
}

#[test]
fn block_comments_are_ignored() {
    assert_eq!(
        run(r#"
            /* block
               spanning two lines */
            const x = /* inline in value position */ 7;
            /* between statements */ const y = 5;
            return x * y;
            "#,),
        json!(35),
    );
}

#[test]
fn url_in_string_literal_is_not_a_comment() {
    assert_eq!(
        run(r#"
            const url = "https://example.com/a//b/*c*/";
            return url;
            "#,),
        json!("https://example.com/a//b/*c*/"),
    );
}

#[test]
fn for_of_array_destructuring_binds_entries() {
    assert_eq!(
        run(r#"
            const pairs = [["a", 1], ["b", 2]];
            let out = "";
            for (const [k, v] of pairs) {
                out += k + v;
            }
            return out;
            "#,),
        json!("a1b2"),
    );
}

#[test]
fn for_of_object_destructuring_binds_fields() {
    assert_eq!(
        run(r#"
            const rows = [{ id: 1 }, { id: 2 }];
            let sum = 0;
            for (const { id } of rows) {
                sum += id;
            }
            return sum;
            "#,),
        json!(3),
    );
}

#[test]
fn const_array_destructuring_binds_positionally() {
    assert_eq!(
        run(r#"
            const pair = [3, 4];
            const [a, b] = pair;
            return a * 10 + b;
            "#,),
        json!(34),
    );
}

#[test]
fn const_object_destructuring_binds_fields() {
    assert_eq!(
        run(r#"
            const point = { x: 5, y: 7 };
            const { x, y } = point;
            return x * 10 + y;
            "#,),
        json!(57),
    );
}

#[test]
fn date_now_returns_increasing_epoch_millis() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let value = run(r#"
            const first = Date.now();
            const second = Date.now();
            return [first, second];
            "#);
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let arr = value.as_array().expect("Date.now must return array");
    let first = arr[0].as_i64().expect("first Date.now must be integer");
    let second = arr[1].as_i64().expect("second Date.now must be integer");
    assert!(first > 0, "epoch millis must be positive: {first}");
    assert!(
        second >= first,
        "Date.now must not go backward: {first} -> {second}"
    );
    let tolerance: i64 = 2000;
    assert!(
        first >= before - tolerance && first <= after + tolerance,
        "first Date.now {first} outside wall-clock bounds [{before}, {after}] + tolerance {tolerance}"
    );
    assert!(
        second >= before - tolerance && second <= after + tolerance,
        "second Date.now {second} outside wall-clock bounds [{before}, {after}] + tolerance {tolerance}"
    );
}

#[test]
fn comments_in_expressions_do_not_become_values() {
    assert_eq!(
        run(r#"
            const args = [1, /* two */ 2, 3];
            const obj = { /* k */ a: 4 };
            const sum = (/* lead */ args[0] + args[1] + args[2] + obj.a);
            const label = String("ok" /* trailing arg comment */);
            const add = (a, /* mid */ b) => a + b;
            return sum === 10 && label === "ok" && add(2, 3) === 5;
            "#,),
        json!(true),
    );
}

#[test]
fn carriage_return_escape_decodes_to_carriage_return() {
    // A carriage-return escape must decode to one carriage-return byte.
    assert_eq!(
        run(r#"
            return "a\rb";
            "#,),
        json!("a\rb"),
    );
}

#[test]
fn full_standard_escape_battery_decodes() {
    assert_eq!(
        run(r#"
            return ["\'", "\"", "\\n", "\t", "\r\n", "\0", "\x41", "\u0041", "\u{1F680}", "\q"];
            "#,),
        json!(["'", "\"", "\\n", "\t", "\r\n", "\u{0}", "A", "A", "🚀", "q",]),
    );
}

#[test]
fn escaped_backslash_then_n_is_not_a_newline() {
    // The two-character sequence backslash-n must not decode as a newline.
    assert_eq!(
        run(r#"
            const value = "\\n";
            return [value.length, value];
            "#,),
        json!([2, "\\n"]),
    );
}

#[test]
fn template_literal_escapes_decode() {
    assert_eq!(
        run(r#"
            return `a\rb\tc${1 + 1}\u{41}`;
            "#,),
        json!("a\rb\tc2A"),
    );
}

#[test]
fn legacy_octal_escape_is_rejected_not_corrupted() {
    let limits = HostLimits::new(
        8 * 1024 * 1024,
        256 * 1024,
        Duration::from_secs(5),
        50_000,
        1,
        2,
        4,
        4 * 1024,
        1024 * 1024,
    )
    .unwrap();
    let registration = GlobalRegistration {
        root: "z".into(),
        capabilities: vec![CapabilityDescriptor::new("z", "read")],
    };
    let err = Host::new_zero_kernel(limits, registration)
        .unwrap()
        .execute(r#"return "\101";"#, Rc::new(UnusedConnector))
        .unwrap_err();
    assert!(
        matches!(err, HostError::UnsupportedSyntax(_)),
        "legacy octal must be rejected as UnsupportedSyntax, got {err:?}"
    );
}

#[test]
fn switch_and_finally_execute_statement_bodies_only() {
    assert_eq!(
        run(r#"
            let branch = "";
            let finalized = false;
            try {
                switch (10) {
                    case 10: branch = "ten"; break;
                    default: branch = "other";
                }
            } finally {
                finalized = true;
            }
            return [branch, finalized];
            "#),
        json!(["ten", true]),
    );
}

#[test]
fn classes_bind_constructor_and_method_this() {
    assert_eq!(
        run(r#"
            class Counter {
                constructor(start) { this.value = start; }
                add(delta) { this.value += delta; return this.value; }
            }
            const counter = new Counter(4);
            return [counter.add(3), counter.value];
            "#),
        json!([7, 7]),
    );
}

#[test]
fn common_string_and_regex_methods_work() {
    assert_eq!(
        run(r#"
            const normalized = "a-b-b".replace("b", "B").replaceAll("b", "B");
            const match = /([A-Z]+)-(\d+)/i.exec("tag-42");
            return [normalized, /TAG/i.test("tag-42"), match[1], match[2]];
            "#),
        json!(["a-B-B", true, "tag", "42"]),
    );
}

#[test]
fn common_array_composition_methods_work() {
    assert_eq!(
        run(r#"
            const values = [1, 2].concat([3, 4], 5);
            return values.reduce((sum, value) => sum + value, 0);
            "#),
        json!(15),
    );
}

#[test]
fn javascript_set_spread_iterates_values() {
    assert_eq!(
        run(r#"
            const values = [...new Set(["a", "a", "b"])];
            return values;
            "#),
        json!(["a", "b"]),
    );
}

#[test]
fn javascript_delete_removes_open_object_property() {
    assert_eq!(
        run(r#"
            const value = { keep: 1, remove: 2 };
            const removed = delete value.remove;
            const missing = delete value.missing;
            return [removed, missing, value];
            "#),
        json!([true, true, { "keep": 1 }]),
    );
}

#[test]
fn map_and_set_support_mutation_iteration_and_callbacks() {
    assert_eq!(
        run(r#"
            const set = new Set(["a", "a", "b"]);
            set.add("c").add("d");
            const removedSet = set.delete("a");
            let setValues = "";
            for (const value of set) {
                setValues += value;
            }

            const map = new Map([["k", 7], ["k", 8]]);
            map.set("z", 9);
            let mapEntries = "";
            for (const [key, value] of map) {
                mapEntries += key + value;
            }
            let visited = "";
            map.forEach((value, key) => { visited += key + value; });
            const removedMap = map.delete("z");

            return [
                set.size,
                set.has("b"),
                removedSet,
                setValues,
                map.size,
                map.get("k"),
                map.has("z"),
                removedMap,
                mapEntries,
                visited,
                JSON.stringify(map),
            ];
            "#),
        json!([
            3, true, true, "bcd", 1, 8, false, true, "k8z9", "k8z9", "{}"
        ]),
    );
}
