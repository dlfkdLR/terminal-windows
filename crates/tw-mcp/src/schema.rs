//! Tool definitions with strict JSON Schemas (`additionalProperties: false`).

use serde_json::{json, Value};

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

pub fn tools() -> Value {
    let vm =
        json!({ "type": "string", "description": "VM name (defaults to the server's VM)", "pattern": "^[A-Za-z0-9][A-Za-z0-9_-]{0,47}$" });
    let px = json!({ "type": "integer", "minimum": 0, "maximum": 16383 });
    let after = json!({ "type": "boolean", "description": "Also return a screenshot taken shortly after the action, for verification." });
    let ro = json!({ "readOnlyHint": true, "openWorldHint": false });
    json!([
        { "name": "screenshot",
          "description": "Capture the Windows guest's screen from the virtual display. All x/y coordinates for other tools are pixels in this image (top-left origin).",
          "inputSchema": obj(json!({ "vm": vm }), &[]), "annotations": ro },
        { "name": "click",
          "description": "Click at a point. clicks: 2 = double-click, 3 = triple-click. button: left (default), right or middle.",
          "inputSchema": obj(json!({ "vm": vm, "x": px, "y": px,
              "button": { "type": "string", "enum": ["left", "right", "middle"] },
              "clicks": { "type": "integer", "minimum": 1, "maximum": 3 }, "screenshot": after }), &["x", "y"]) },
        { "name": "move",
          "description": "Move the pointer, e.g. to reveal a tooltip or hover menu.",
          "inputSchema": obj(json!({ "vm": vm, "x": px, "y": px, "screenshot": after }), &["x", "y"]) },
        { "name": "drag",
          "description": "Press the left button at (x, y), move to (to_x, to_y) and release.",
          "inputSchema": obj(json!({ "vm": vm, "x": px, "y": px, "to_x": px, "to_y": px, "screenshot": after }), &["x", "y", "to_x", "to_y"]) },
        { "name": "scroll",
          "description": "Scroll at a point. 120 units = one wheel notch. Negative dy scrolls content down, positive up; dx scrolls horizontally.",
          "inputSchema": obj(json!({ "vm": vm, "x": px, "y": px,
              "dx": { "type": "integer", "minimum": -10000, "maximum": 10000 },
              "dy": { "type": "integer", "minimum": -10000, "maximum": 10000 }, "screenshot": after }), &["x", "y", "dy"]) },
        { "name": "type",
          "description": "Type text into the focused control. Without the guest agent only US-keyboard ASCII is supported; use `key` for Enter, Tab and shortcuts.",
          "inputSchema": obj(json!({ "vm": vm, "text": { "type": "string", "minLength": 1, "maxLength": 32768 }, "screenshot": after }), &["text"]) },
        { "name": "key",
          "description": "Press a key or chord, e.g. return, tab, escape, win, win+r, ctrl+shift+esc, alt+f4, ctrl+c. Modifiers: ctrl, shift, alt, win.",
          "inputSchema": obj(json!({ "vm": vm, "key": { "type": "string", "minLength": 1, "maxLength": 64 }, "screenshot": after }), &["key"]) },
        { "name": "status",
          "description": "Report the VM state and which control plane is in use.",
          "inputSchema": obj(json!({ "vm": vm }), &[]), "annotations": ro },
        { "name": "list_vms",
          "description": "List the Windows VMs and their states.",
          "inputSchema": obj(json!({}), &[]), "annotations": ro },
        { "name": "start_vm",
          "description": "Start (boot) a stopped VM. Returns once the hypervisor reports it running; Windows itself takes longer to boot — take screenshots to follow progress.",
          "inputSchema": obj(json!({ "vm": vm }), &[]) },
        { "name": "stop_vm",
          "description": "Request a graceful shutdown (ACPI power button). force=true powers off immediately and can lose unsaved work; only use it if the user asked.",
          "inputSchema": obj(json!({ "vm": vm, "force": { "type": "boolean" } }), &[]), "annotations": { "destructiveHint": true } }
    ])
}
