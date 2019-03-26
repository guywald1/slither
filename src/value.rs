use crate::interpreter::{AssemblerFunctionInfo, Context, Interpreter, Scope};
use crate::intrinsics::{perform_await, promise::new_promise_capability};
use crate::parser::FunctionKind;
use crate::{Agent, IntoValue};
use gc::{Gc, GcCell};
use indexmap::IndexMap;
use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};

type BuiltinFunction = fn(&Agent, Vec<Value>, &Context) -> Result<Value, Value>;

static SYMBOL_COUNTER: AtomicUsize = AtomicUsize::new(0);
#[derive(Debug, Clone, Trace, Finalize, Hash, PartialEq, Eq)]
pub struct Symbol(pub usize, pub bool, pub Option<String>); // id, private, description

impl Symbol {
    pub fn new(private: bool, desc: Option<String>) -> Symbol {
        let s = Symbol(SYMBOL_COUNTER.load(Ordering::Relaxed), private, desc);
        SYMBOL_COUNTER.fetch_add(1, Ordering::Relaxed);
        s
    }
}

#[derive(Trace, Finalize, Debug, Eq, Clone)]
pub enum ObjectKey {
    Number(usize),
    String(String),
    Symbol(Symbol),
}

impl PartialEq for ObjectKey {
    fn eq(&self, other: &Self) -> bool {
        match self {
            ObjectKey::Number(n) => match other {
                ObjectKey::Number(nv) => n == nv,
                ObjectKey::String(s) => &n.to_string() == s,
                ObjectKey::Symbol(..) => false,
            },
            ObjectKey::String(s) => match other {
                ObjectKey::String(sv) => s == sv,
                ObjectKey::Number(n) => &n.to_string() == s,
                ObjectKey::Symbol(..) => false,
            },
            ObjectKey::Symbol(s) => match other {
                ObjectKey::Symbol(sv) => s == sv,
                _ => false,
            },
        }
    }
}

impl PartialOrd for ObjectKey {
    fn partial_cmp(&self, other: &ObjectKey) -> Option<std::cmp::Ordering> {
        match self {
            ObjectKey::Number(n) => match other {
                ObjectKey::Number(nv) => n.partial_cmp(nv),
                ObjectKey::String(s) => n.to_string().partial_cmp(s),
                ObjectKey::Symbol(..) => Some(std::cmp::Ordering::Less),
            },
            ObjectKey::String(s) => match other {
                ObjectKey::String(sv) => s.partial_cmp(sv),
                ObjectKey::Number(n) => n.to_string().partial_cmp(s),
                ObjectKey::Symbol(..) => Some(std::cmp::Ordering::Less),
            },
            ObjectKey::Symbol(..) => match other {
                ObjectKey::Symbol(..) => Some(std::cmp::Ordering::Equal),
                _ => Some(std::cmp::Ordering::Greater),
            },
        }
    }
}

impl Ord for ObjectKey {
    fn cmp(&self, other: &ObjectKey) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap()
    }
}

impl Hash for ObjectKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            ObjectKey::Number(n) => {
                0.hash(state);
                n.to_string().hash(state);
            }
            ObjectKey::String(s) => {
                1.hash(state);
                s.hash(state);
            }
            ObjectKey::Symbol(s) => {
                2.hash(state);
                s.hash(state);
            }
        }
    }
}

impl std::fmt::Display for ObjectKey {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ObjectKey::Number(n) => write!(fmt, "{}", n),
            ObjectKey::String(s) => write!(fmt, "{}", s),
            ObjectKey::Symbol(Symbol(_, _, d)) => {
                if let Some(s) = d {
                    write!(fmt, "[Symbol({})]", s)
                } else {
                    write!(fmt, "[Symbol()]")
                }
            }
        }
    }
}

impl From<String> for ObjectKey {
    fn from(s: String) -> Self {
        ObjectKey::String(s)
    }
}

impl From<&str> for ObjectKey {
    fn from(s: &str) -> Self {
        ObjectKey::String(s.to_string())
    }
}

impl From<u32> for ObjectKey {
    fn from(n: u32) -> Self {
        ObjectKey::Number(n as usize)
    }
}

impl From<i32> for ObjectKey {
    fn from(n: i32) -> Self {
        if n >= 0 {
            ObjectKey::Number(n as usize)
        } else {
            ObjectKey::String(n.to_string())
        }
    }
}

impl From<usize> for ObjectKey {
    fn from(n: usize) -> Self {
        ObjectKey::Number(n)
    }
}

impl From<f64> for ObjectKey {
    fn from(n: f64) -> Self {
        if n >= 0f64 {
            ObjectKey::Number(n as usize)
        } else {
            ObjectKey::String(n.to_string())
        }
    }
}

#[derive(Finalize)]
pub enum ObjectKind {
    Ordinary,
    Array,
    Boolean(bool),
    String(String),
    Number(f64),
    Regex(Regex),
    Buffer(GcCell<Vec<u8>>),
    BytecodeFunction {
        kind: FunctionKind,
        parameters: Vec<String>,
        position: usize,
        scope: Gc<GcCell<Scope>>,
    },
    BuiltinFunction(BuiltinFunction, GcCell<HashMap<String, Value>>),
    Custom(GcCell<HashMap<String, Value>>),
}

unsafe impl gc::Trace for ObjectKind {
    custom_trace!(this, {
        match this {
            ObjectKind::BytecodeFunction { scope, .. } => {
                mark(scope);
            }
            ObjectKind::Custom(slots) | ObjectKind::BuiltinFunction(_, slots) => {
                mark(slots);
            }
            _ => {}
        }
    });
}

impl std::fmt::Debug for ObjectKind {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        let r = match self {
            ObjectKind::Ordinary => "Ordinary".to_string(),
            ObjectKind::Array => "Array".to_string(),
            ObjectKind::Boolean(b) => format!("Boolean({})", b),
            ObjectKind::String(s) => format!("String({})", s),
            ObjectKind::Number(i) => format!("Number({})", i),
            ObjectKind::Regex(r) => format!("Regex({})", r),
            ObjectKind::Buffer(b) => format!("Buffer({:?})", b),
            ObjectKind::Custom(..) => "Custom".to_string(),
            ObjectKind::BytecodeFunction { position, .. } => {
                format!("CompiledFunction @ {}", position)
            }
            ObjectKind::BuiltinFunction(f, ..) => format!("BuiltinFunction @ {:p}", f),
        };
        write!(fmt, "{}", r)
    }
}

#[derive(Debug, Trace, Finalize)]
pub struct ObjectInfo {
    pub kind: ObjectKind,
    properties: GcCell<IndexMap<ObjectKey, Value>>,
    prototype: Value,
}

impl ObjectInfo {
    fn get(&self, property: ObjectKey) -> Value {
        match self.properties.borrow().get(&property) {
            Some(v) => v.clone(),
            _ => {
                if let ObjectKey::Symbol(Symbol(_, true, _)) = property {
                    // don't traverse for private symbol
                    Value::Null
                } else {
                    match &self.prototype {
                        Value::Object(oo) => oo.get(property),
                        Value::Null => Value::Null,
                        _ => unreachable!(),
                    }
                }
            }
        }
    }

    pub fn set(
        &self,
        agent: &Agent,
        property: ObjectKey,
        value: Value,
        receiver: Gc<ObjectInfo>,
    ) -> Result<Value, Value> {
        let own = if let ObjectKey::Symbol(Symbol(_, true, _)) = property {
            true
        } else {
            false
        };
        if own || self.properties.borrow().contains_key(&property) {
            receiver
                .properties
                .borrow_mut()
                .insert(property, value.clone());
            Ok(value)
        } else {
            match &self.prototype {
                Value::Object(oo) => oo.set(agent, property, value, receiver),
                Value::Null => {
                    receiver
                        .properties
                        .borrow_mut()
                        .insert(property, value.clone());
                    Ok(value)
                }
                _ => unreachable!(),
            }
        }
    }

    fn keys(&self) -> Vec<ObjectKey> {
        let mut keys = Vec::new();
        let entries = self.properties.borrow();
        for key in entries.keys() {
            if let ObjectKey::Symbol(Symbol(_, true, ..)) = key {
                // private keys are unenumerable
            } else {
                keys.push(key.clone());
            }
        }
        keys.sort();
        keys
    }
}

#[derive(Debug, Finalize, Clone)]
pub enum Value {
    // Language types
    Null,
    True,
    False,
    String(String),
    Number(f64),
    Symbol(Symbol),
    Object(Gc<ObjectInfo>),
    Tuple(Vec<Value>),

    // Internal types
    Empty,
    List(GcCell<VecDeque<Value>>),
    WrappedContext(Gc<GcCell<Context>>, Option<Box<Value>>),
    Iterator(Box<Value>, Box<Value>),
}

unsafe impl gc::Trace for Value {
    custom_trace!(this, {
        match this {
            Value::Null
            | Value::True
            | Value::False
            | Value::String(_)
            | Value::Number(_)
            | Value::Symbol(_) => {}
            Value::Object(o) => mark(o),
            Value::Tuple(items, ..) => mark(items),

            Value::Empty => {}
            Value::List(list) => mark(list),
            Value::WrappedContext(c, p) => {
                mark(c);
                mark(p);
            }
            Value::Iterator(i, n) => {
                mark(i);
                mark(n);
            }
        }
    });
}

impl Value {
    pub fn new_symbol(desc: Option<String>) -> Value {
        Value::Symbol(Symbol::new(false, desc))
    }

    pub fn new_private_symbol(desc: Option<String>) -> Value {
        Value::Symbol(Symbol::new(true, desc))
    }

    pub fn new_object(prototype: Value) -> Value {
        Value::Object(Gc::new(ObjectInfo {
            kind: ObjectKind::Ordinary,
            properties: GcCell::new(IndexMap::new()),
            prototype,
        }))
    }

    pub fn new_custom_object(prototype: Value) -> Value {
        Value::Object(Gc::new(ObjectInfo {
            kind: ObjectKind::Custom(GcCell::new(HashMap::new())),
            properties: GcCell::new(IndexMap::new()),
            prototype,
        }))
    }

    pub fn new_error(agent: &Agent, message: &str) -> Value {
        let mut properties = IndexMap::new();
        properties.insert(
            ObjectKey::from("message"),
            Value::String(message.to_string()),
        );
        Value::Object(Gc::new(ObjectInfo {
            kind: ObjectKind::Ordinary,
            properties: GcCell::new(properties),
            prototype: agent.intrinsics.error_prototype.clone(),
        }))
    }

    pub fn new_array(agent: &Agent) -> Value {
        Value::Object(Gc::new(ObjectInfo {
            kind: ObjectKind::Array,
            properties: GcCell::new(IndexMap::new()),
            prototype: agent.intrinsics.array_prototype.clone(),
        }))
    }

    pub fn new_regex_object(agent: &Agent, r: &str) -> Result<Value, Value> {
        let re = match Regex::new(r) {
            Ok(r) => r,
            Err(e) => {
                return Err(Value::new_error(agent, &format!("{}", e)));
            }
        };
        Ok(Value::Object(Gc::new(ObjectInfo {
            kind: ObjectKind::Regex(re),
            properties: GcCell::new(IndexMap::new()),
            prototype: agent.intrinsics.regex_prototype.clone(),
        })))
    }

    pub fn new_buffer_from_vec(agent: &Agent, vec: Vec<u8>) -> Value {
        Value::Object(Gc::new(ObjectInfo {
            kind: ObjectKind::Buffer(GcCell::new(vec)),
            properties: GcCell::new(IndexMap::new()),
            prototype: agent.intrinsics.array_prototype.clone(),
        }))
    }

    pub fn new_list() -> Value {
        Value::List(GcCell::new(VecDeque::new()))
    }

    pub fn new_tuple() -> Value {
        Value::Tuple(Vec::new())
    }

    pub fn new_bytecode_function(
        agent: &Agent,
        info: &AssemblerFunctionInfo,
        scope: Gc<GcCell<Scope>>,
    ) -> Value {
        Value::Object(Gc::new(ObjectInfo {
            kind: ObjectKind::BytecodeFunction {
                kind: info.kind,
                position: info.position,
                parameters: info.parameters.clone(),
                scope,
            },
            properties: GcCell::new(IndexMap::new()),
            prototype: agent.intrinsics.function_prototype.clone(),
        }))
    }

    pub fn new_builtin_function(agent: &Agent, f: BuiltinFunction) -> Value {
        Value::Object(Gc::new(ObjectInfo {
            kind: ObjectKind::BuiltinFunction(f, GcCell::new(HashMap::new())),
            properties: GcCell::new(IndexMap::new()),
            prototype: agent.intrinsics.function_prototype.clone(),
        }))
    }

    pub fn new_iter_result(agent: &Agent, value: Value, done: bool) -> Result<Value, Value> {
        let o = Value::new_object(agent.intrinsics.object_prototype.clone());
        o.set(agent, ObjectKey::from("value"), value)?;
        o.set(
            agent,
            ObjectKey::from("done"),
            if done { Value::True } else { Value::False },
        )?;
        Ok(o)
    }
}

impl Value {
    pub fn type_of(&self) -> &str {
        match &self {
            Value::Null => "null",
            Value::True => "boolean",
            Value::False => "boolean",
            Value::Number(..) => "number",
            Value::String(..) => "string",
            Value::Symbol(..) => "symbol",
            Value::Object(o) => match o.kind {
                ObjectKind::BytecodeFunction { .. } => "function",
                ObjectKind::BuiltinFunction(..) => "function",
                _ => "object",
            },
            Value::Tuple(..) => "tuple",
            _ => unreachable!(),
        }
    }

    pub fn to_bool(&self) -> bool {
        match &self {
            Value::Null => false,
            Value::True => true,
            Value::False => false,
            Value::Number(n) => *n != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::Symbol(..) => true,
            Value::Object(..) => true,
            Value::Tuple(..) => true,
            _ => unreachable!(),
        }
    }

    pub fn get(&self, agent: &Agent, key: ObjectKey) -> Result<Value, Value> {
        match self {
            Value::Object(o) => Ok(o.get(key)),
            Value::Tuple(t, ..) => match key {
                ObjectKey::Number(n) => Ok(t.get(n).unwrap_or(&Value::Null).clone()),
                _ => Ok(Value::Null),
            },
            _ => self.to_object(agent)?.get(agent, key),
        }
    }

    pub fn set(&self, agent: &Agent, key: ObjectKey, value: Value) -> Result<Value, Value> {
        match self {
            Value::Object(o) => o.set(agent, key, value, o.clone()),
            _ => Err(Value::new_error(agent, "base must be an object")),
        }
    }

    pub fn keys(&self, agent: &Agent) -> Result<Vec<ObjectKey>, Value> {
        match self {
            Value::Object(o) => Ok(o.keys()),
            Value::Tuple(vec) => Ok((0..vec.len())
                .map(ObjectKey::from)
                .collect::<Vec<ObjectKey>>()),
            _ => Err(Value::new_error(agent, "base must be an object")),
        }
    }

    pub fn get_slot(&self, key: &str) -> Value {
        if let Value::Object(o) = self {
            match &o.kind {
                ObjectKind::Custom(slots) | ObjectKind::BuiltinFunction(_, slots) => {
                    match slots.borrow().get(key) {
                        Some(v) => v.clone(),
                        _ => panic!(),
                    }
                }
                _ => panic!(),
            }
        } else {
            panic!()
        }
    }

    pub fn set_slot(&self, key: &str, value: Value) {
        if let Value::Object(o) = self {
            match &o.kind {
                ObjectKind::Custom(slots) | ObjectKind::BuiltinFunction(_, slots) => {
                    slots.borrow_mut().insert(key.to_string(), value);
                }
                _ => panic!(),
            }
        } else {
            panic!()
        }
    }

    pub fn has_slot(&self, property: &str) -> bool {
        if let Value::Object(o) = self {
            match &o.kind {
                ObjectKind::Custom(slots) | ObjectKind::BuiltinFunction(_, slots) => {
                    slots.borrow().contains_key(property)
                }
                _ => false,
            }
        } else {
            false
        }
    }

    pub fn to_object(&self, agent: &Agent) -> Result<Value, Value> {
        match self {
            Value::Null => Err(Value::new_error(agent, "cannot convert null to object")),
            Value::True | Value::False => Ok(Value::Object(Gc::new(ObjectInfo {
                kind: ObjectKind::Boolean(*self == Value::True),
                properties: GcCell::new(IndexMap::new()),
                prototype: agent.intrinsics.boolean_prototype.clone(),
            }))),
            Value::Object(_) => Ok(self.clone()),
            Value::Number(n) => Ok(Value::Object(Gc::new(ObjectInfo {
                kind: ObjectKind::Number(*n),
                properties: GcCell::new(IndexMap::new()),
                prototype: agent.intrinsics.number_prototype.clone(),
            }))),
            Value::String(s) => Ok(Value::Object(Gc::new(ObjectInfo {
                kind: ObjectKind::String(s.to_string()),
                properties: GcCell::new(IndexMap::new()),
                prototype: agent.intrinsics.string_prototype.clone(),
            }))),
            Value::Tuple(_) => Ok(self.clone()),
            _ => unreachable!(),
        }
    }

    pub fn to_object_key(&self, agent: &Agent) -> Result<ObjectKey, Value> {
        match self {
            Value::Symbol(s) => Ok(ObjectKey::Symbol(s.clone())),
            Value::String(s) => Ok(ObjectKey::from(s.to_string())),
            Value::Number(n) => Ok(ObjectKey::from(*n)),
            _ => Err(Value::new_error(agent, "cannot convert to object key")),
        }
    }

    pub fn call(&self, agent: &Agent, this: Value, args: Vec<Value>) -> Result<Value, Value> {
        match self {
            Value::Object(o) => match &o.kind {
                ObjectKind::BytecodeFunction {
                    position,
                    kind,
                    scope,
                    parameters,
                    ..
                } => {
                    let ctx = Context::new(Scope::new(Some(scope.clone())));
                    if *kind & FunctionKind::Arrow == FunctionKind::Arrow {
                        // FIXME: doesn't have `this` vs inherited `this` needs to be clarified
                    } else {
                        ctx.borrow().scope.borrow_mut().this = Some(if this == Value::Null {
                            Value::Null
                        } else {
                            this.to_object(agent)?
                        });
                    }
                    ctx.borrow_mut().function = Some(self.clone());
                    evaluate_body(agent, ctx, *position, *kind, args, parameters)
                }
                ObjectKind::BuiltinFunction(f, ..) => {
                    let c = Context::new(Scope::new(None));
                    let mut b = c.borrow_mut();
                    b.scope.borrow_mut().this = Some(if this == Value::Null {
                        Value::Null
                    } else {
                        this.to_object(agent)?
                    });
                    b.function = Some(self.clone());
                    f(agent, args, &b)
                }
                _ => Err(Value::new_error(agent, "value is not a function")),
            },
            _ => Err(Value::new_error(agent, "value is not a function")),
        }
    }

    pub fn construct(
        &self,
        agent: &Agent,
        args: Vec<Value>,
        new_target: Value,
    ) -> Result<Value, Value> {
        match self {
            Value::Object(o) => match &o.kind {
                ObjectKind::BytecodeFunction {
                    position,
                    kind,
                    scope,
                    parameters,
                    ..
                } => {
                    if *kind != FunctionKind::Normal
                        || (*kind & FunctionKind::Arrow == FunctionKind::Arrow)
                    {
                        Err(Value::new_error(agent, "value is not a constructor"))
                    } else {
                        let mut prototype = new_target.get(agent, ObjectKey::from("prototype"))?;
                        if prototype.type_of() != "object" {
                            prototype = agent.intrinsics.object_prototype.clone();
                        }
                        let this = Value::new_object(prototype);
                        let ctx = Context::new(Scope::new(Some(scope.clone())));
                        ctx.borrow().scope.borrow_mut().this = Some(this.clone());
                        ctx.borrow_mut().function = Some(self.clone());
                        let r = evaluate_body(agent, ctx, *position, *kind, args, parameters)?;
                        if r.type_of() == "object" {
                            Ok(r)
                        } else {
                            Ok(this)
                        }
                    }
                }
                ObjectKind::BuiltinFunction(f, ..) => {
                    let mut prototype = new_target.get(agent, ObjectKey::from("prototype"))?;
                    if prototype.type_of() != "object" {
                        prototype = agent.intrinsics.object_prototype.clone();
                    }
                    let this = Value::new_object(prototype);
                    let c = Context::new(Scope::new(None));
                    let mut cb = c.borrow_mut();
                    cb.scope.borrow_mut().this = Some(this.clone());
                    cb.function = Some(self.clone());
                    let r = f(agent, args, &cb)?;
                    if r.type_of() == "object" {
                        Ok(r)
                    } else {
                        Ok(this)
                    }
                }
                _ => Err(Value::new_error(agent, "value is not a function")),
            },
            _ => Err(Value::new_error(agent, "value is not a function")),
        }
    }

    #[inline]
    pub fn inspect(agent: &Agent, value: &Value) -> String {
        inspect(agent, value, 0, &mut HashSet::new())
    }
}

fn evaluate_body(
    agent: &Agent,
    ctx: Gc<GcCell<Context>>,
    position: usize,
    kind: FunctionKind,
    args: Vec<Value>,
    params: &[String],
) -> Result<Value, Value> {
    for (i, param) in params.iter().enumerate() {
        ctx.borrow().scope.borrow_mut().create(param, false);
        ctx.borrow()
            .scope
            .borrow_mut()
            .initialize(param, args.get(i).unwrap_or(&Value::Null).clone());
    }

    let mut interpreter = Interpreter::new(position, ctx.clone());

    if kind & FunctionKind::Normal == FunctionKind::Normal {
        interpreter.run(agent).unwrap()
    } else if kind & FunctionKind::Generator == FunctionKind::Generator {
        ctx.borrow_mut().interpreter = Some(interpreter);
        let o = Value::new_custom_object(agent.intrinsics.generator_prototype.clone());
        o.set_slot("generator context", Value::WrappedContext(ctx, None));
        Ok(o)
    } else if kind & FunctionKind::Async == FunctionKind::Async {
        let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;
        match interpreter.run(agent) {
            Ok(r) => match r {
                Ok(v) => {
                    promise
                        .get_slot("resolve")
                        .call(agent, Value::Null, vec![v])?;
                }
                Err(e) => {
                    promise
                        .get_slot("reject")
                        .call(agent, Value::Null, vec![e])?;
                }
            },
            Err(mut c) => {
                ctx.borrow_mut().interpreter = Some(interpreter);
                let value = std::mem::replace(&mut c.0, Value::Null);
                perform_await(
                    agent,
                    Value::WrappedContext(ctx, Some(Box::new(promise.clone()))),
                    value,
                )?;
            }
        }
        Ok(promise)
    } else {
        unreachable!();
    }
}

#[inline]
pub fn ref_eq<T>(thing: &T, other: &T) -> bool {
    (thing as *const T) == (other as *const T)
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match self {
            Value::Null => match other {
                Value::Null => true,
                _ => false,
            },
            Value::True => match other {
                Value::True => true,
                _ => false,
            },
            Value::False => match other {
                Value::False => true,
                _ => false,
            },
            Value::String(s) => match &other {
                Value::String(vs) => s == vs,
                _ => false,
            },
            Value::Number(n) => match &other {
                Value::Number(vn) => n == vn,
                _ => false,
            },
            Value::Symbol(s) => match &other {
                Value::Symbol(vs) => s == vs,
                _ => false,
            },
            Value::Object(o) => match &other {
                Value::Object(vo) => ref_eq(&*o.properties.borrow(), &*vo.properties.borrow()),
                _ => false,
            },
            Value::Tuple(i) => match &other {
                Value::Tuple(vi) => {
                    i.len() == vi.len() && i.iter().enumerate().all(|(i, v)| &vi[i] == v)
                }
                _ => false,
            },
            Value::Empty => match other {
                Value::Empty => true,
                _ => false,
            },

            Value::List(..) | Value::WrappedContext(..) | Value::Iterator(..) => {
                ref_eq(self, other)
            }
        }
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Value::Null => 0.hash(state),
            Value::True => 1.hash(state),
            Value::False => 2.hash(state),
            Value::String(s) => {
                3.hash(state);
                s.hash(state);
            }
            Value::Number(n) => {
                4.hash(state);
                n.to_bits().hash(state);
            }
            Value::Symbol(s) => {
                5.hash(state);
                s.hash(state);
            }
            Value::Object(o) => {
                6.hash(state);
                // hash the memory address of the map sigh
                (&*o.properties.borrow() as *const IndexMap<ObjectKey, Value>).hash(state);
            }
            Value::Tuple(items) => {
                7.hash(state);
                items.hash(state);
            }
            _ => unreachable!(),
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

impl IntoValue for std::net::AddrParseError {
    fn into_value(&self, agent: &Agent) -> Value {
        Value::new_error(agent, &format!("{}", self))
    }
}

impl IntoValue for std::io::Error {
    fn into_value(&self, agent: &Agent) -> Value {
        Value::new_error(agent, &format!("{}", self))
    }
}

fn inspect(
    agent: &Agent,
    value: &Value,
    indent: usize,
    inspected: &mut HashSet<*const IndexMap<ObjectKey, Value>>,
) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::True => "true".to_string(),
        Value::False => "false".to_string(),
        Value::Number(n) => format!("{}", n),
        Value::String(s) => format!("'{}'", s),
        Value::Symbol(Symbol(_, _, d)) => {
            if let Some(s) = d {
                format!("Symbol({})", s)
            } else {
                "Symbol()".to_string()
            }
        }
        Value::Tuple(items) => {
            let mut ins = Vec::new();
            for item in items {
                ins.push(inspect(agent, item, indent, inspected));
            }
            format!("({})", ins.join(", "))
        }
        Value::Object(o) => {
            if let ObjectKind::Regex(re) = &o.kind {
                return format!("/{}/", re);
            }
            if o.prototype == agent.intrinsics.error_prototype {
                if let Ok(Value::String(s)) =
                    o.get(ObjectKey::from("toString"))
                        .call(agent, value.clone(), vec![])
                {
                    return s;
                }
            }
            let hash_key = &*o.properties.borrow() as *const IndexMap<ObjectKey, Value>;
            if inspected.contains(&hash_key) {
                "[Circular]".to_string()
            } else {
                inspected.insert(hash_key);
                let array = match o.kind {
                    ObjectKind::Array => true,
                    _ => false,
                };
                let function = value.type_of() == "function";
                let keys = value.keys(agent).unwrap();
                let mut out = String::new();
                if function {
                    out += "[Function";
                    if let Value::String(name) = o.get(ObjectKey::from("name")) {
                        out += " ";
                        out += name.as_str();
                        if keys.len() == 1 {
                            out += "]";
                            return out;
                        }
                    }
                    out += "]";
                    if keys.is_empty() {
                        return out;
                    }
                }
                out += if array { "[" } else { "{" };
                if keys.is_empty() {
                    out += if array { "]" } else { "}" };
                    return out;
                }
                for key in keys {
                    if function && key == ObjectKey::from("name") {
                        continue;
                    }
                    out += &format!(
                        "\n{}{}: {},",
                        "  ".repeat(indent + 1),
                        key.clone(),
                        inspect(
                            agent,
                            &value.get(agent, key).unwrap(),
                            indent + 1,
                            inspected
                        )
                    )
                }
                out += &format!("\n{}{}", "  ".repeat(indent), if array { "]" } else { "}" });
                out
            }
        }
        v => unreachable!("{:?}", v),
    }
}
