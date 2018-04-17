use std;
use nes::{ErrorInfo,ErrorInfoTrait};
use sender;
use nanomsg;

use ::ThreadSource;


define_error!( Error,
    HandlerThreadCrash(thread_source:ThreadSource) =>
        "[Source:{1}] Handler thread has finished incorrecty(crashed)",
    BalancerCrash(thread_source:ThreadSource) =>
        "[Source:{1}] Balancer server has crashed",
    BalancerCrashed(sender_error:Box<sender::Error>) =>
        "Balancer server has crashed: {1}",

    BrockenChannel() =>
        "Broken channel",
    Poisoned() =>
        "Mutex has been poisoned",

    NanomsgError(nanomsg_error:Box<nanomsg::result::Error>) =>
        "Nanomsg error: {}",
    IOError(io_error:Box<std::io::Error>) =>
        "IO Error: {}",

    Other(message:String) =>
        "{}"
);