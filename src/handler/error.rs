use std;
use nes::{ErrorInfo,ErrorInfoTrait};
use sender;

use ::ThreadSource;

define_error!( Error,
    IpcListenerThreadCrash(thread_source:ThreadSource) =>
        "[Source:{1}] IpcListener thread has finished incorrecty(crashed)",
    BalancerCrash(thread_source:ThreadSource) =>
        "[Source:{1}] Balancer server has crashed",
    BalancerCrashed(sender_error:Box<sender::Error>) =>
        "Balancer server has crashed: {1}",

    BrockenChannel() =>
        "Broken channel",
    Poisoned() =>
        "Mutex has been poisoned",

    Other(message:String) =>
        "{}"
);