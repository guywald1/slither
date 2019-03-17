use crate::agent::Agent;
use crate::value::Value;
use crate::vm::ExecutionContext;

fn iterator(agent: &Agent, ctx: &ExecutionContext, _: Vec<Value>) -> Result<Value, Value> {
    ctx.environment.borrow().get_this(agent)
}

pub fn create_async_iterator_prototype(agent: &Agent) -> Value {
    let proto = Value::new_object(agent.intrinsics.object_prototype.clone());

    proto
        .set(
            agent,
            &agent
                .well_known_symbol("asyncIterator")
                .to_object_key(agent)
                .unwrap(),
            Value::new_builtin_function(agent, iterator),
        )
        .unwrap();

    proto
}
