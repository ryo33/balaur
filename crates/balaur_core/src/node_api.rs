//! The node API, declared once for every language.
//!
//! Each operation takes the node as its first argument, so a backend can
//! register these as free functions or bind them as methods on its own node
//! handle — see `NODE_OPS`. Adding a language costs the sugar, not the
//! twenty-odd operations.

// Every declaration shares one signature so they can sit in a table of
// function pointers; several of them have nothing to fail at.
#![allow(clippy::unnecessary_wraps)]

use anyhow::{anyhow, bail, Result};
use balaur_script::{Bindings, Value};
use glamx::{EulerRot, Quat, Vec3};
use hecs::Entity;

use crate::engine::{Command, Engine};
use crate::eure_value::EureValue;
use crate::scene::{self, Children, GlobalTransform, Name, Parent, ScriptAttachment, Transform};

/// One node operation, as a plain function pointer so the list stays a `const`.
pub struct NodeOp {
    pub name: &'static str,
    pub call: fn(&Engine, &[Value]) -> Result<Value>,
}

/// Every node operation, in one list.
pub const NODE_OPS: &[NodeOp] = &[
    NodeOp {
        name: "is_valid",
        call: is_valid,
    },
    NodeOp {
        name: "name",
        call: name,
    },
    NodeOp {
        name: "set_name",
        call: set_name,
    },
    NodeOp {
        name: "path",
        call: path,
    },
    NodeOp {
        name: "position",
        call: position,
    },
    NodeOp {
        name: "set_position",
        call: set_position,
    },
    NodeOp {
        name: "translate",
        call: translate,
    },
    NodeOp {
        name: "rotation_euler",
        call: rotation_euler,
    },
    NodeOp {
        name: "set_rotation_euler",
        call: set_rotation_euler,
    },
    NodeOp {
        name: "rotation_degrees",
        call: rotation_degrees,
    },
    NodeOp {
        name: "set_rotation_degrees",
        call: set_rotation_degrees,
    },
    NodeOp {
        name: "scale",
        call: scale,
    },
    NodeOp {
        name: "set_scale",
        call: set_scale,
    },
    NodeOp {
        name: "global_position",
        call: global_position,
    },
    NodeOp {
        name: "global_rotation_euler",
        call: global_rotation_euler,
    },
    NodeOp {
        name: "global_scale",
        call: global_scale,
    },
    NodeOp {
        name: "get_node",
        call: get_node,
    },
    NodeOp {
        name: "add_child",
        call: add_child,
    },
    NodeOp {
        name: "parent",
        call: parent,
    },
    NodeOp {
        name: "children",
        call: children,
    },
    NodeOp {
        name: "set_parent",
        call: set_parent,
    },
    NodeOp {
        name: "set_component",
        call: set_component,
    },
    NodeOp {
        name: "remove_component",
        call: remove_component,
    },
    NodeOp {
        name: "get_component",
        call: get_component,
    },
    NodeOp {
        name: "has_component",
        call: has_component,
    },
    NodeOp {
        name: "component_names",
        call: component_names,
    },
    NodeOp {
        name: "script_path",
        call: script_path,
    },
    NodeOp { name: "call", call },
    NodeOp {
        name: "attach_script",
        call: attach_script,
    },
    NodeOp {
        name: "detach_script",
        call: detach_script,
    },
    NodeOp {
        name: "queue_free",
        call: queue_free,
    },
];

/// Register every node operation into a binding group as a free function.
///
/// A backend that gives its node handle method syntax walks `NODE_OPS`
/// itself instead; this is the plain path.
pub fn install_node_api(m: &mut dyn Bindings<Engine>) {
    m.module_doc(
        "What every node has: its name and path, its transform in local and \
         world space, its children, its components and its script. Each \
         operation takes the node as its first argument, so scripts normally \
         call them as methods on a node value (`this.node.position()`).",
    );
    m.describe(&[
        ("is_valid", &[], "()", "Whether the node is still in the world; false rather than an error when the value is not a node."),
        ("name", &[], "()", "The node's own name, empty when it carries none."),
        ("set_name", &[], "(name: string)", "Rename the node, doing nothing when it carries no name."),
        ("path", &[], "()", "The node's slash-separated path, built by climbing parents from the node up to the root."),
        ("position", &[], "()", "The node's position in its parent's space."),
        ("set_position", &[], "(x: float, y: float, z: float)", "Move the node to a local position, given as three numbers or one vector."),
        ("translate", &[], "(x: float, y: float, z: float)", "Move the node by an offset in its parent's space, given as three numbers or one vector."),
        ("rotation_euler", &[], "()", "The node's local rotation as euler angles in radians, x then y then z."),
        ("set_rotation_euler", &[], "(x: float, y: float, z: float)", "Set the node's local rotation from euler angles in radians."),
        ("rotation_degrees", &[], "()", "The same local rotation as `rotation_euler`, in degrees."),
        ("set_rotation_degrees", &[], "(x: float, y: float, z: float)", "Set the node's local rotation from euler angles in degrees."),
        ("scale", &[], "()", "The node's scale relative to its parent."),
        ("set_scale", &[], "(x: float, y: float, z: float)", "Set the node's scale relative to its parent, as three numbers or one vector."),
        ("global_position", &[], "()", "The node's position in world space, as of the last transform sync."),
        ("global_rotation_euler", &[], "()", "The node's world rotation as euler angles in radians, as of the last transform sync."),
        ("global_scale", &[], "()", "The node's scale in world space, as of the last transform sync."),
        ("get_node", &[], "(path: string)", "The node at an `A/B/C` path relative to this one, `..` climbing to the parent; nil when nothing matches."),
        ("add_child", &[], "(name: string)", "Create a named child node under this one and return it."),
        ("parent", &[], "()", "The node's parent, nil at the root."),
        ("children", &[], "()", "The node's direct children, an empty list when it has none."),
        ("set_parent", &[], "(parent: node)", "Move the node under another, keeping where it is in the world; an error for a cycle or a dead parent."),
        ("set_component", &[], "(component: string, params: any?)", "Give the node the named component, built from the given table over the component's schema defaults."),
        ("remove_component", &[], "(component: string)", "Take the named component off the node."),
        ("get_component", &[], "(component: string)", "The named component's properties as a table, nil when the node does not carry it."),
        ("has_component", &[], "(component: string)", "Whether the node carries the named component."),
        ("component_names", &[], "()", "The names of every component on the node."),
        ("script_path", &[], "()", "The path of the script attached to the node, nil when it has none."),
        ("call", &[], "(method: string, args: any?)", "Call a method on the node's script and return what it gives back; nil when there is no such script or method."),
        ("attach_script", &[], "(path: string, props: any?)", "Attach the script at a path, with an optional table overriding what the script exports."),
        ("detach_script", &[], "()", "Drop the script instance on this node, so no further lifecycle call reaches it; the node and its components stay."),
        ("queue_free", &[], "()", "Destroy the node and its subtree at the end of the frame."),
    ]);
    for d in NODE_OPS {
        m.function_raw(d.name, Box::new(d.call));
    }
}

fn node(args: &[Value]) -> Result<Entity> {
    match args.first() {
        Some(Value::Node(id)) => crate::entity_of(balaur_script::NodeId(*id)),
        _ => Err(anyhow!("expected a node as the first argument")),
    }
}

fn text(args: &[Value], i: usize) -> Result<&str> {
    match args.get(i) {
        Some(Value::Str(s)) => Ok(s),
        other => Err(anyhow!("argument {i} should be a string, got {other:?}")),
    }
}

fn number(args: &[Value], i: usize) -> Result<f32> {
    match args.get(i) {
        Some(Value::Num(n)) => Ok(*n as f32),
        Some(Value::Int(n)) => Ok(*n as f32),
        other => Err(anyhow!("argument {i} should be a number, got {other:?}")),
    }
}

/// Read three numbers, or one vector, so `set_position(v)` and
/// `set_position(x, y, z)` both work.
fn xyz(args: &[Value], from: usize) -> Result<Vec3> {
    if let Some(Value::Vec3([x, y, z])) = args.get(from) {
        return Ok(Vec3::new(*x, *y, *z));
    }
    Ok(Vec3::new(
        number(args, from)?,
        number(args, from + 1)?,
        number(args, from + 2)?,
    ))
}

fn vec3(v: Vec3) -> Value {
    Value::Vec3([v.x, v.y, v.z])
}

fn with_transform<R>(eng: &Engine, e: Entity, f: impl FnOnce(&mut Transform) -> R) -> Result<R> {
    let world = eng.world();
    let mut transform = world
        .get::<&mut Transform>(e)
        .map_err(|_| anyhow!("node is dead or has no transform"))?;
    Ok(f(&mut transform))
}

fn is_valid(eng: &Engine, args: &[Value]) -> Result<Value> {
    let Ok(e) = node(args) else {
        return Ok(Value::Bool(false));
    };
    Ok(Value::Bool(eng.world().contains(e)))
}

fn name(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let world = eng.world();
    Ok(Value::Str(
        world
            .get::<&Name>(e)
            .map(|n| n.0.clone())
            .unwrap_or_default(),
    ))
}

fn set_name(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let world = eng.world();
    if let Ok(mut n) = world.get::<&mut Name>(e) {
        n.0 = text(args, 1)?.to_string();
    }
    Ok(Value::Nil)
}

fn path(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    Ok(Value::Str(scene::node_path(&eng.world(), e)))
}

fn position(eng: &Engine, args: &[Value]) -> Result<Value> {
    with_transform(eng, node(args)?, |t| vec3(t.position))
}

fn set_position(eng: &Engine, args: &[Value]) -> Result<Value> {
    let v = xyz(args, 1)?;
    with_transform(eng, node(args)?, |t| t.position = v)?;
    Ok(Value::Nil)
}

fn translate(eng: &Engine, args: &[Value]) -> Result<Value> {
    let v = xyz(args, 1)?;
    with_transform(eng, node(args)?, |t| t.position += v)?;
    Ok(Value::Nil)
}

fn rotation_euler(eng: &Engine, args: &[Value]) -> Result<Value> {
    with_transform(eng, node(args)?, |t| {
        let (yaw, pitch, roll) = t.rotation.to_euler(EulerRot::ZYX);
        Value::Vec3([roll, pitch, yaw])
    })
}

fn set_rotation_euler(eng: &Engine, args: &[Value]) -> Result<Value> {
    let v = xyz(args, 1)?;
    with_transform(eng, node(args)?, |t| {
        t.rotation = Quat::from_euler(EulerRot::ZYX, v.z, v.y, v.x);
    })?;
    Ok(Value::Nil)
}

/// The same rotation as `rotation_euler`, in degrees.
///
/// Radians are the engine's unit and stay the default; degrees are what a
/// person authors, so the pair exists rather than every caller carrying its
/// own `math.deg` conversion the way the editor's inspector used to.
fn rotation_degrees(eng: &Engine, args: &[Value]) -> Result<Value> {
    with_transform(eng, node(args)?, |t| {
        let (yaw, pitch, roll) = t.rotation.to_euler(EulerRot::ZYX);
        Value::Vec3([roll.to_degrees(), pitch.to_degrees(), yaw.to_degrees()])
    })
}

fn set_rotation_degrees(eng: &Engine, args: &[Value]) -> Result<Value> {
    let v = xyz(args, 1)?;
    with_transform(eng, node(args)?, |t| {
        t.rotation = Quat::from_euler(
            EulerRot::ZYX,
            v.z.to_radians(),
            v.y.to_radians(),
            v.x.to_radians(),
        );
    })?;
    Ok(Value::Nil)
}

fn scale(eng: &Engine, args: &[Value]) -> Result<Value> {
    with_transform(eng, node(args)?, |t| vec3(t.scale))
}

fn set_scale(eng: &Engine, args: &[Value]) -> Result<Value> {
    let v = xyz(args, 1)?;
    with_transform(eng, node(args)?, |t| t.scale = v)?;
    Ok(Value::Nil)
}

fn global<R>(eng: &Engine, args: &[Value], f: impl FnOnce(&GlobalTransform) -> R) -> Result<R> {
    let e = node(args)?;
    let world = eng.world();
    let g = world
        .get::<&GlobalTransform>(e)
        .map_err(|_| anyhow!("node is dead"))?;
    Ok(f(&g))
}

fn global_position(eng: &Engine, args: &[Value]) -> Result<Value> {
    global(eng, args, |g| vec3(g.position))
}

fn global_scale(eng: &Engine, args: &[Value]) -> Result<Value> {
    global(eng, args, |g| vec3(g.scale))
}

fn global_rotation_euler(eng: &Engine, args: &[Value]) -> Result<Value> {
    global(eng, args, |g| {
        let (yaw, pitch, roll) = g.rotation.to_euler(EulerRot::ZYX);
        Value::Vec3([roll, pitch, yaw])
    })
}

fn get_node(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let world = eng.world();
    Ok(scene::find_node(&world, e, text(args, 1)?)
        .map_or(Value::Nil, |found| Value::Node(crate::node_id_of(found).0)))
}

fn add_child(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let mut world = eng.world_mut();
    let child = scene::spawn_node(&mut world, text(args, 1)?, e);
    crate::ids::assign(eng, &mut world, child);
    Ok(Value::Node(crate::node_id_of(child).0))
}

fn parent(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let world = eng.world();
    Ok(world
        .get::<&Parent>(e)
        .ok()
        .map_or(Value::Nil, |p| Value::Node(crate::node_id_of(p.0).0)))
}

/// `node:set_parent(other)`: move the node under another, keeping where it
/// is in the world.
fn set_parent(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let parent = match args.get(1) {
        Some(Value::Node(id)) => crate::entity_of(balaur_script::NodeId(*id))?,
        other => return Err(anyhow!("argument 1 should be a node, got {other:?}")),
    };
    scene::reparent(&mut eng.world_mut(), e, parent)?;
    Ok(Value::Nil)
}

fn children(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let world = eng.world();
    let out = world.get::<&Children>(e).map_or_else(
        |_| Vec::new(),
        |c| {
            c.0.iter()
                .map(|&child| Value::Node(crate::node_id_of(child).0))
                .collect()
        },
    );
    Ok(Value::List(out))
}

/// Adds the component if the node lacks it, merges over it if it has it, so
/// one verb covers both. There is deliberately no `add_component`: the family
/// reads set_ / get_ / has_ / remove_.
fn set_component(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let params = match args.get(2) {
        None | Some(Value::Nil) => None,
        Some(v) => Some(to_toml(v)?),
    };
    crate::components::add(eng, e, text(args, 1)?, params.as_ref())?;
    Ok(Value::Nil)
}

fn remove_component(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    crate::components::remove(eng, e, text(args, 1)?)?;
    Ok(Value::Nil)
}

fn get_component(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    crate::components::get(eng, e, text(args, 1)?)
        .as_ref()
        .map_or(Ok(Value::Nil), from_toml)
}

fn has_component(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    Ok(Value::Bool(
        crate::components::get(eng, e, text(args, 1)?).is_some(),
    ))
}

fn component_names(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    Ok(Value::List(
        crate::components::present_on(eng, e)
            .into_iter()
            .map(Value::Str)
            .collect(),
    ))
}

fn script_path(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let world = eng.world();
    Ok(world
        .get::<&ScriptAttachment>(e)
        .ok()
        .map_or(Value::Nil, |a| Value::Str(a.path.clone())))
}

/// `node:call("method", ...)` — one script calling another's method, with
/// the target's return value coming back. Nil when the node has no script,
/// no such method (handlers are opt-in), or the method suspended on an
/// await; the call itself runs to completion before this returns, so a
/// handler may spawn, free or call further nodes.
fn call(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let method = text(args, 1)?;
    let host = eng
        .script_host()
        .ok_or_else(|| anyhow!("no script backend is running"))?;
    Ok(host
        .call_on(crate::node_id_of(e), method, args.get(2..).unwrap_or(&[]))
        .unwrap_or(Value::Nil))
}

/// `node:attach_script(path, props)` — the scene's `script` key, at run time.
/// `props` is optional and holds what this node overrides of the script's
/// exported defaults, so a spawned node is tuned the way an authored one is.
fn attach_script(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let props = match args.get(2) {
        None | Some(Value::Nil) => Vec::new(),
        Some(Value::Map(fields)) => fields.clone(),
        Some(other) => bail!(
            "attach_script props must be a table, got {}",
            other.type_name()
        ),
    };
    let host = eng
        .script_host()
        .ok_or_else(|| anyhow!("no script backend is running"))?;
    host.attach_with_props(crate::node_id_of(e), text(args, 1)?, &props)?;
    Ok(Value::Nil)
}

/// `node:detach_script()` — drop the instance, keeping the node.
///
/// A tool that tears a running scene down needs the scripts to stop before
/// the world does: an instance whose next `update` runs against a world its
/// own components have left throws where nothing is wrong.
fn detach_script(eng: &Engine, args: &[Value]) -> Result<Value> {
    let e = node(args)?;
    let host = eng
        .script_host()
        .ok_or_else(|| anyhow!("no script backend is running"))?;
    host.detach(crate::node_id_of(e));
    Ok(Value::Nil)
}

fn queue_free(eng: &Engine, args: &[Value]) -> Result<Value> {
    eng.push_command(Command::Free(node(args)?));
    Ok(Value::Nil)
}

/// Component parameters travel as TOML, so a script table and a scene file
/// describe a component the same way.
///
/// This stays TOML-shaped rather than [`EureValue`] on purpose: it is the
/// wire format between `balaur_core` and every plugin's component `apply`
/// hooks (`balaur_physics`, `balaur_render`, `balaur_ui`, `balaur_anim`, ...),
/// which is a much larger boundary than the project/scene loader this module
/// otherwise serves. [`to_eure`]/[`from_eure`] are the ones scene loading
/// itself uses.
pub fn to_toml(v: &Value) -> Result<toml::Value> {
    Ok(match v {
        Value::Nil => toml::Value::String(String::new()),
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Int(i) => toml::Value::Integer(*i),
        Value::Num(n) => toml::Value::Float(*n),
        Value::Str(s) => toml::Value::String(s.clone()),
        Value::Node(_) | Value::Callback(_) => {
            return Err(anyhow!("a node or callback is not component data"))
        }
        Value::Many(_) => return Err(anyhow!("several values are not component data")),
        // TOML has no byte string, and a component that wanted one would be
        // asking for an asset reference instead.
        Value::Bytes(_) => return Err(anyhow!("bytes are not component data")),
        Value::Vec2(a) => number_list(a),
        Value::Vec3(a) => number_list(a),
        Value::Color(a) => number_list(a),
        Value::List(items) => toml::Value::Array(items.iter().map(to_toml).collect::<Result<_>>()?),
        Value::Map(pairs) => toml::Value::Table(
            pairs
                .iter()
                .map(|(k, val)| Ok((k.clone(), to_toml(val)?)))
                .collect::<Result<_>>()?,
        ),
    })
}

fn number_list(a: &[f32]) -> toml::Value {
    toml::Value::Array(
        a.iter()
            .map(|n| toml::Value::Float(f64::from(*n)))
            .collect(),
    )
}

pub fn from_toml(v: &toml::Value) -> Result<Value> {
    Ok(match v {
        toml::Value::String(s) => Value::Str(s.clone()),
        toml::Value::Integer(i) => Value::Int(*i),
        toml::Value::Float(f) => Value::Num(*f),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => Value::Str(d.to_string()),
        toml::Value::Array(items) => {
            Value::List(items.iter().map(from_toml).collect::<Result<_>>()?)
        }
        toml::Value::Table(table) => Value::Map(
            table
                .iter()
                .map(|(k, val)| Ok((k.clone(), from_toml(val)?)))
                .collect::<Result<_>>()?,
        ),
    })
}

/// The [`EureValue`] equivalent of [`to_toml`], for scene/project loading —
/// the part of the engine that reads `.eure` documents directly rather than
/// going through the toml-shaped component `apply` boundary.
pub fn to_eure(v: &Value) -> Result<EureValue> {
    Ok(match v {
        Value::Nil => EureValue::string(String::new()),
        Value::Bool(b) => EureValue::bool(*b),
        Value::Int(i) => EureValue::integer(*i),
        Value::Num(n) => EureValue::float(*n),
        Value::Str(s) => EureValue::string(s.clone()),
        Value::Node(_) | Value::Callback(_) => {
            return Err(anyhow!("a node or callback is not component data"))
        }
        Value::Many(_) => return Err(anyhow!("several values are not component data")),
        Value::Bytes(_) => return Err(anyhow!("bytes are not component data")),
        Value::Vec2(a) => EureValue::array(a.iter().map(|n| EureValue::float(f64::from(*n)))),
        Value::Vec3(a) => EureValue::array(a.iter().map(|n| EureValue::float(f64::from(*n)))),
        Value::Color(a) => EureValue::array(a.iter().map(|n| EureValue::float(f64::from(*n)))),
        Value::List(items) => EureValue::array(
            items
                .iter()
                .map(to_eure)
                .collect::<Result<Vec<_>>>()?,
        ),
        Value::Map(pairs) => EureValue::table(
            pairs
                .iter()
                .map(|(k, val)| Ok((k.clone(), to_eure(val)?)))
                .collect::<Result<Vec<_>>>()?,
        ),
    })
}

/// The [`EureValue`] equivalent of [`from_toml`].
pub fn from_eure(v: &EureValue) -> Result<Value> {
    if let Some(s) = v.as_str() {
        return Ok(Value::Str(s.to_string()));
    }
    if let Some(b) = v.as_bool() {
        return Ok(Value::Bool(b));
    }
    if v.is_table() {
        return Ok(Value::Map(
            v.iter_table()
                .map(|(k, val)| Ok((k, from_eure(&val)?)))
                .collect::<Result<_>>()?,
        ));
    }
    if v.is_array() {
        return Ok(Value::List(
            v.as_array_items()
                .iter()
                .map(from_eure)
                .collect::<Result<_>>()?,
        ));
    }
    if let Some(i) = v.as_integer() {
        return Ok(Value::Int(i));
    }
    if let Some(f) = v.as_float() {
        return Ok(Value::Num(f));
    }
    Ok(Value::Nil)
}

/// Renders a [`toml::Value`] table as Eure source text, for writing a
/// definition back to a `.eure` file (`crate::assets::save`).
///
/// Every value is written as an object/array literal (`{ "k" => v, ... }` /
/// `[a, b]`) rather than section syntax (`@ k`): literals nest arbitrarily
/// without needing to track when a section closes, which is what a definition
/// written back by a tool — shape unknown ahead of time — actually needs.
#[must_use]
pub fn toml_to_eure_text(root: &toml::Value) -> String {
    let toml::Value::Table(table) = root else {
        // Every asset definition is a table; nothing here ever hands this a
        // bare value, so falling back to a single root binding is defensive
        // rather than a path any caller takes.
        return format!("= {}\n", eure_literal(root));
    };
    let mut out = String::new();
    for (key, value) in table {
        out.push_str(&eure_key(key));
        out.push_str(" = ");
        out.push_str(&eure_literal(value));
        out.push('\n');
    }
    out
}

fn eure_literal(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => eure_string(s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => {
            let mut s = f.to_string();
            if !s.contains(['.', 'e', 'E']) {
                s.push_str(".0");
            }
            s
        }
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => eure_string(&d.to_string()),
        toml::Value::Array(items) => {
            let items: Vec<String> = items.iter().map(eure_literal).collect();
            format!("[{}]", items.join(", "))
        }
        toml::Value::Table(table) => {
            let entries: Vec<String> = table
                .iter()
                .map(|(k, v)| format!("{} => {}", eure_key(k), eure_literal(v)))
                .collect();
            format!("{{ {} }}", entries.join(", "))
        }
    }
}

fn eure_key(key: &str) -> String {
    eure_string(key)
}

fn eure_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A [`toml::Value`] built from an [`EureValue`], for the boundary between
/// `.eure` scene/asset documents and the still-TOML-shaped component and
/// asset-definition system (see [`to_toml`]'s note).
pub fn eure_to_toml(v: &EureValue) -> toml::Value {
    if let Some(s) = v.as_str() {
        return toml::Value::String(s.to_string());
    }
    if let Some(b) = v.as_bool() {
        return toml::Value::Boolean(b);
    }
    if v.is_table() {
        return toml::Value::Table(
            v.iter_table()
                .map(|(k, val)| (k, eure_to_toml(&val)))
                .collect(),
        );
    }
    if v.is_array() {
        return toml::Value::Array(v.as_array_items().iter().map(eure_to_toml).collect());
    }
    if let Some(i) = v.as_integer() {
        return toml::Value::Integer(i);
    }
    if let Some(f) = v.as_float() {
        return toml::Value::Float(f);
    }
    toml::Value::String(String::new())
}
