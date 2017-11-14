use handler;

use ::ThreadSource;

use handler::HandlerSender;
use ::ArcTasksQueue;
use ::ArcSender;
use ::ArcAutomat;

pub enum IpcListenerCommand {
    HandlerThreadCrash(ThreadSource),
    BalancerCrash(ThreadSource),

    HandlerSender(HandlerSender),
    TasksQueue(ArcTasksQueue),
    Sender(ArcSender),
    Automat(ArcAutomat),
    SenderCreationError,
    HandlerSetupError(Box<handler::Error>),
    HandlerIsReady,
    Shutdown,
    HandlerFinished
}