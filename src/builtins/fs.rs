use crate::agent::{Agent, MioMapType};
use crate::interpreter::Context;
use crate::intrinsics::promise::new_promise_capability;
use crate::value::{ObjectKey, Value};
use lazy_static::lazy_static;
use mio::{PollOpt, Ready, Registration, Token};
use std::collections::HashMap;
use std::sync::Mutex;

lazy_static! {
    static ref RESPONSES: Mutex<HashMap<Token, FsResponse>> = Mutex::new(HashMap::new());
}

pub enum FsResponse {
    Read(String),
    Metadata(std::fs::Metadata),
    Exists(bool),
    Success,
    Error(String),
}

pub fn handle(agent: &Agent, token: Token, promise: Value) {
    let fsr = RESPONSES.lock().unwrap().remove(&token).unwrap();
    match fsr {
        FsResponse::Read(s) => {
            promise
                .get_slot("resolve")
                .call(agent, promise, vec![Value::from(s)])
                .unwrap();
        }
        FsResponse::Metadata(m) => {
            let o = Value::new_object(agent.intrinsics.object_prototype.clone());
            macro_rules! p {
                ($target:expr, $name:expr, $value:expr) => {
                    $target.set(agent, ObjectKey::from($name), $value).unwrap();
                };
            }
            let ft = m.file_type();
            if ft.is_file() {
                p!(o, "type", Value::from("file"));
            } else if ft.is_dir() {
                p!(o, "type", Value::from("directory"));
            } else if ft.is_symlink() {
                p!(o, "type", Value::from("symlink"));
            } else {
                unreachable!();
            }
            p!(o, "size", Value::from(m.len() as f64));
            macro_rules! t {
                ($name:expr, $value:expr) => {
                    let d = $value
                        .unwrap()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap();
                    let seconds = d.as_secs();
                    let subsec_millis = u64::from(d.subsec_millis());
                    let ms = seconds * 1000 + subsec_millis;
                    p!(o, $name, Value::from(ms as f64));
                };
            }
            t!("modifiedAt", m.modified());
            t!("accessedAt", m.accessed());
            t!("createdAt", m.created());

            let permissions = Value::new_object(agent.intrinsics.object_prototype.clone());
            p!(
                permissions,
                "read",
                Value::from(!m.permissions().readonly())
            );
            p!(o, "permissions", permissions);

            promise
                .get_slot("resolve")
                .call(agent, promise, vec![o])
                .unwrap();
        }
        FsResponse::Exists(exists) => {
            promise
                .get_slot("resolve")
                .call(agent, promise, vec![Value::from(exists)])
                .unwrap();
        }
        FsResponse::Success => {
            promise
                .get_slot("resolve")
                .call(agent, promise, vec![])
                .unwrap();
        }
        FsResponse::Error(s) => {
            promise
                .get_slot("reject")
                .call(agent, promise, vec![Value::new_error(agent, s.as_str())])
                .unwrap();
        }
    }
}

fn read_file(agent: &Agent, args: Vec<Value>, _: &Context) -> Result<Value, Value> {
    if let Some(Value::String(filename)) = args.get(0) {
        let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;

        let (registration, set_readiness) = Registration::new2();
        let token = Token(agent.mio_map.borrow().len());

        agent
            .mio
            .register(&registration, token, Ready::readable(), PollOpt::edge())
            .unwrap();
        agent
            .mio_map
            .borrow_mut()
            .insert(token, MioMapType::FS(registration, promise.clone()));

        let filename = filename.to_string();
        agent
            .pool
            .execute(move || match std::fs::read_to_string(filename) {
                Ok(s) => {
                    RESPONSES.lock().unwrap().insert(token, FsResponse::Read(s));
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
                Err(e) => {
                    RESPONSES
                        .lock()
                        .unwrap()
                        .insert(token, FsResponse::Error(format!("{}", e)));
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
            });

        Ok(promise)
    } else {
        Err(Value::new_error(agent, "filename must be a string"))
    }
}

fn write_file(agent: &Agent, args: Vec<Value>, _: &Context) -> Result<Value, Value> {
    if let Some(Value::String(filename)) = args.get(0) {
        if let Some(Value::String(contents)) = args.get(1) {
            let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;

            let (registration, set_readiness) = Registration::new2();
            let token = Token(agent.mio_map.borrow().len());

            agent
                .mio
                .register(&registration, token, Ready::readable(), PollOpt::edge())
                .unwrap();
            agent
                .mio_map
                .borrow_mut()
                .insert(token, MioMapType::FS(registration, promise.clone()));

            let filename = filename.to_string();
            let contents = contents.to_string();
            agent
                .pool
                .execute(move || match std::fs::write(filename, contents) {
                    Ok(()) => {
                        RESPONSES.lock().unwrap().insert(token, FsResponse::Success);
                        set_readiness.set_readiness(Ready::readable()).unwrap();
                    }
                    Err(e) => {
                        RESPONSES
                            .lock()
                            .unwrap()
                            .insert(token, FsResponse::Error(format!("{}", e)));
                        set_readiness.set_readiness(Ready::readable()).unwrap();
                    }
                });

            Ok(promise)
        } else {
            Err(Value::new_error(agent, "contents must be a string"))
        }
    } else {
        Err(Value::new_error(agent, "filename must be a string"))
    }
}

fn remove_file(agent: &Agent, args: Vec<Value>, _: &Context) -> Result<Value, Value> {
    if let Some(Value::String(filename)) = args.get(0) {
        let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;

        let (registration, set_readiness) = Registration::new2();
        let token = Token(agent.mio_map.borrow().len());

        agent
            .mio
            .register(&registration, token, Ready::readable(), PollOpt::edge())
            .unwrap();
        agent
            .mio_map
            .borrow_mut()
            .insert(token, MioMapType::FS(registration, promise.clone()));

        let filename = filename.to_string();
        agent
            .pool
            .execute(move || match std::fs::remove_file(filename) {
                Ok(()) => {
                    RESPONSES.lock().unwrap().insert(token, FsResponse::Success);
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
                Err(e) => {
                    RESPONSES
                        .lock()
                        .unwrap()
                        .insert(token, FsResponse::Error(format!("{}", e)));
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
            });

        Ok(promise)
    } else {
        Err(Value::new_error(agent, "filename must be a string"))
    }
}

fn get_metadata(agent: &Agent, args: Vec<Value>, _: &Context) -> Result<Value, Value> {
    if let Some(Value::String(filename)) = args.get(0) {
        let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;

        let (registration, set_readiness) = Registration::new2();
        let token = Token(agent.mio_map.borrow().len());

        agent
            .mio
            .register(&registration, token, Ready::readable(), PollOpt::edge())
            .unwrap();
        agent
            .mio_map
            .borrow_mut()
            .insert(token, MioMapType::FS(registration, promise.clone()));

        let filename = filename.to_string();
        agent
            .pool
            .execute(move || match std::fs::metadata(filename) {
                Ok(metadata) => {
                    RESPONSES
                        .lock()
                        .unwrap()
                        .insert(token, FsResponse::Metadata(metadata));
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
                Err(e) => {
                    RESPONSES
                        .lock()
                        .unwrap()
                        .insert(token, FsResponse::Error(format!("{}", e)));
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
            });

        Ok(promise)
    } else {
        Err(Value::new_error(agent, "filename must be a string"))
    }
}

fn copy(agent: &Agent, args: Vec<Value>, _: &Context) -> Result<Value, Value> {
    if let Some(Value::String(from)) = args.get(0) {
        if let Some(Value::String(to)) = args.get(1) {
            let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;

            let (registration, set_readiness) = Registration::new2();
            let token = Token(agent.mio_map.borrow().len());

            agent
                .mio
                .register(&registration, token, Ready::readable(), PollOpt::edge())
                .unwrap();
            agent
                .mio_map
                .borrow_mut()
                .insert(token, MioMapType::FS(registration, promise.clone()));

            let from = from.to_string();
            let to = to.to_string();
            agent.pool.execute(move || match std::fs::copy(from, to) {
                Ok(_) => {
                    RESPONSES.lock().unwrap().insert(token, FsResponse::Success);
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
                Err(e) => {
                    RESPONSES
                        .lock()
                        .unwrap()
                        .insert(token, FsResponse::Error(format!("{}", e)));
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
            });

            Ok(promise)
        } else {
            Err(Value::new_error(agent, "to must be a string"))
        }
    } else {
        Err(Value::new_error(agent, "from must be a string"))
    }
}

fn move_(agent: &Agent, args: Vec<Value>, _: &Context) -> Result<Value, Value> {
    if let Some(Value::String(from)) = args.get(0) {
        if let Some(Value::String(to)) = args.get(1) {
            let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;

            let (registration, set_readiness) = Registration::new2();
            let token = Token(agent.mio_map.borrow().len());

            agent
                .mio
                .register(&registration, token, Ready::readable(), PollOpt::edge())
                .unwrap();
            agent
                .mio_map
                .borrow_mut()
                .insert(token, MioMapType::FS(registration, promise.clone()));

            let from = from.to_string();
            let to = to.to_string();
            agent.pool.execute(move || match std::fs::rename(from, to) {
                Ok(_) => {
                    RESPONSES.lock().unwrap().insert(token, FsResponse::Success);
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
                Err(e) => {
                    RESPONSES
                        .lock()
                        .unwrap()
                        .insert(token, FsResponse::Error(format!("{}", e)));
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
            });

            Ok(promise)
        } else {
            Err(Value::new_error(agent, "to must be a string"))
        }
    } else {
        Err(Value::new_error(agent, "from must be a string"))
    }
}

#[cfg(windows)]
fn symlink(from: String, to: String) -> std::io::Result<()> {
    if std::fs::metadata(&from)?.is_file() {
        std::os::windows::fs::symlink_file(from, to)
    } else {
        std::os::windows::fs::symlink_dir(from, to)
    }
}

#[cfg(not(windows))]
fn symlink(from: String, to: String) -> std::io::Result<()> {
    std::os::unix::fs::symlink(from, to)
}

fn create_symlink(agent: &Agent, args: Vec<Value>, _: &Context) -> Result<Value, Value> {
    if let Some(Value::String(from)) = args.get(0) {
        if let Some(Value::String(to)) = args.get(1) {
            let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;

            let (registration, set_readiness) = Registration::new2();
            let token = Token(agent.mio_map.borrow().len());

            agent
                .mio
                .register(&registration, token, Ready::readable(), PollOpt::edge())
                .unwrap();
            agent
                .mio_map
                .borrow_mut()
                .insert(token, MioMapType::FS(registration, promise.clone()));

            let from = from.to_string();
            let to = to.to_string();
            agent.pool.execute(move || match symlink(from, to) {
                Ok(()) => {
                    RESPONSES.lock().unwrap().insert(token, FsResponse::Success);
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
                Err(e) => {
                    RESPONSES
                        .lock()
                        .unwrap()
                        .insert(token, FsResponse::Error(format!("{}", e)));
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
            });

            Ok(promise)
        } else {
            Err(Value::new_error(agent, "to must be a string"))
        }
    } else {
        Err(Value::new_error(agent, "from must be a string"))
    }
}

fn exists(agent: &Agent, args: Vec<Value>, _: &Context) -> Result<Value, Value> {
    if let Some(Value::String(filename)) = args.get(0) {
        let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;

        let (registration, set_readiness) = Registration::new2();
        let token = Token(agent.mio_map.borrow().len());

        agent
            .mio
            .register(&registration, token, Ready::readable(), PollOpt::edge())
            .unwrap();
        agent
            .mio_map
            .borrow_mut()
            .insert(token, MioMapType::FS(registration, promise.clone()));

        let filename = filename.to_string();
        agent.pool.execute(move || {
            let exists = std::path::Path::new(filename.as_str()).exists();
            RESPONSES
                .lock()
                .unwrap()
                .insert(token, FsResponse::Exists(exists));
            set_readiness.set_readiness(Ready::readable()).unwrap();
        });

        Ok(promise)
    } else {
        Err(Value::new_error(agent, "filename must be a string"))
    }
}

fn create_directory(agent: &Agent, args: Vec<Value>, _: &Context) -> Result<Value, Value> {
    if let Some(Value::String(filename)) = args.get(0) {
        let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;

        let (registration, set_readiness) = Registration::new2();
        let token = Token(agent.mio_map.borrow().len());

        agent
            .mio
            .register(&registration, token, Ready::readable(), PollOpt::edge())
            .unwrap();
        agent
            .mio_map
            .borrow_mut()
            .insert(token, MioMapType::FS(registration, promise.clone()));

        let filename = filename.to_string();
        agent
            .pool
            .execute(move || match std::fs::create_dir(filename) {
                Ok(()) => {
                    RESPONSES.lock().unwrap().insert(token, FsResponse::Success);
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
                Err(e) => {
                    RESPONSES
                        .lock()
                        .unwrap()
                        .insert(token, FsResponse::Error(format!("{}", e)));
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
            });

        Ok(promise)
    } else {
        Err(Value::new_error(agent, "filename must be a string"))
    }
}

fn remove_directory(agent: &Agent, args: Vec<Value>, _: &Context) -> Result<Value, Value> {
    if let Some(Value::String(filename)) = args.get(0) {
        let promise = new_promise_capability(agent, agent.intrinsics.promise.clone())?;

        let (registration, set_readiness) = Registration::new2();
        let token = Token(agent.mio_map.borrow().len());

        agent
            .mio
            .register(&registration, token, Ready::readable(), PollOpt::edge())
            .unwrap();
        agent
            .mio_map
            .borrow_mut()
            .insert(token, MioMapType::FS(registration, promise.clone()));

        let filename = filename.to_string();
        agent
            .pool
            .execute(move || match std::fs::remove_dir(filename) {
                Ok(()) => {
                    RESPONSES.lock().unwrap().insert(token, FsResponse::Success);
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
                Err(e) => {
                    RESPONSES
                        .lock()
                        .unwrap()
                        .insert(token, FsResponse::Error(format!("{}", e)));
                    set_readiness.set_readiness(Ready::readable()).unwrap();
                }
            });

        Ok(promise)
    } else {
        Err(Value::new_error(agent, "filename must be a string"))
    }
}

pub fn create(agent: &Agent) -> HashMap<String, Value> {
    let mut module = HashMap::new();

    macro_rules! method {
        ($name:expr, $fn:ident) => {
            module.insert($name.to_string(), Value::new_builtin_function(agent, $fn));
        };
    }
    method!("readFile", read_file);
    method!("writeFile", write_file);
    method!("removeFile", remove_file);
    method!("getMetadata", get_metadata);
    method!("copy", copy);
    method!("move", move_);
    method!("createSymbolicLink", create_symlink);
    method!("exists", exists);
    // watch
    method!("createDirectory", create_directory);
    method!("removeDirectory", remove_directory);
    // readDirectory

    module
}
