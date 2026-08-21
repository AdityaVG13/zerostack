use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value as JsonValue, json};
use zero_abi::{CapabilityDescriptor, GlobalRegistration};
use zero_codemode::{
    Connector, ConnectorCompletion, ConnectorError, DispatchContext, Host, HostLimits,
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
    assert_eq!(
        run(r#"
            const first = Date.now();
            const second = Date.now();
            return first > 0 && second >= first;
            "#,),
        json!(true),
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
