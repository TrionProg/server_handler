use std;


use std::sync::{Arc,Mutex,RwLock};

pub type ArcTasksQueue=Arc<TasksQueue>;

pub struct TasksQueue {
}

impl TasksQueue {
    pub fn new() -> Self {
        TasksQueue {

        }
    }

    pub fn new_arc() -> ArcTasksQueue {
        Arc::new( Self::new() )
    }

    pub fn is_task(&self) -> bool {
        false
    }
}
