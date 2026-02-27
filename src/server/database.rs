use omnipaxos_kv::common::kv::KVCommand;
use std::collections::HashMap;

pub enum CommandResult {
    Write,
    Read(Option<String>),
    Cas(bool),
}

pub struct Database {
    db: HashMap<String, String>,
}

impl Database {
    pub fn new() -> Self {
        Self { db: HashMap::new() }
    }

    pub fn handle_command(&mut self, command: KVCommand) -> CommandResult {
        match command {
            KVCommand::Put(key, value) => {
                self.db.insert(key, value);
                CommandResult::Write
            }
            KVCommand::Delete(key) => {
                self.db.remove(&key);
                CommandResult::Write
            }
            KVCommand::Get(key) => CommandResult::Read(self.db.get(&key).cloned()),
            KVCommand::Cas(key, from, to) => {
                let applied = self.db.get(&key).is_some_and(|curr| curr == &from);
                if applied {
                    self.db.insert(key, to);
                }
                CommandResult::Cas(applied)
            }
        }
    }
}
