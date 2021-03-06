use crate::agent::Agent;
use crate::interpreter::Context;
use crate::value::{ObjectKey, Value};

fn to_string(agent: &Agent, _: Vec<Value>, ctx: &Context) -> Result<Value, Value> {
    let this = ctx.scope.borrow().get_this(agent)?;

    let name = match this.get(agent, ObjectKey::from("name"))? {
        Value::String(s) => s,
        _ => return Err(Value::new_error(agent, "Invalid error object")),
    };
    let message = match this.get(agent, ObjectKey::from("message"))? {
        Value::String(s) => format!(": {}", s),
        Value::Null => "".to_string(),
        _ => return Err(Value::new_error(agent, "Invalid error object")),
    };

    Ok(Value::from(format!("{}{}", name, message)))
}

pub fn create_error_prototype(agent: &Agent) -> Value {
    let proto = Value::new_object(agent.intrinsics.object_prototype.clone());

    proto
        .set(
            agent,
            ObjectKey::from("toString"),
            Value::new_builtin_function(agent, to_string),
        )
        .unwrap();

    proto
        .set(agent, ObjectKey::from("name"), Value::from("Error"))
        .unwrap();

    proto
}
