use ipc_listener;
use sender;

use common_messages::MessageServerID;
use ::{ServerType,ServerID};

pub enum HandlerCommand {
    IpcListenerThreadCrash(Box<ipc_listener::Error>),
    BalancerCrash(Box<sender::Error>),

    IpcListenerSetupError(Box<ipc_listener::Error>),
    IpcListenerIsReady,
    ShutdownReceived,
    Shutdown,
    IpcListenerFinished,
    Task,

    //From IPC Listener
    Familiarity(Box<(Vec<(MessageServerID,String)>,Vec<(MessageServerID,String)>,usize)>),
    AcceptConnection(ServerType,ServerID,String,ServerID),
    ConnectionAccepted(ServerType,ServerID,ServerID),
    Connected(ServerType,ServerID),

    SenderCommand(SenderCommand),
}

//From Sender
pub enum SenderCommand {
    ConnectionFailed(ServerType, ServerID, sender::Error),
    AcceptConnectionFailed(ServerType, ServerID, sender::Error),
    TransactionFailed(ServerType, ServerID, sender::Error, sender::BasicState),
    Connected(ServerType, ServerID, ServerID, ServerID),
    ConnectedToAll(ServerType)
}

impl sender::SenderCommand for HandlerCommand{
    fn connection_failed(server_type:ServerType, balancer_server_id:ServerID, error:sender::Error) -> Self {
        HandlerCommand::SenderCommand( SenderCommand::ConnectionFailed(server_type, balancer_server_id, error) )
    }

    fn accept_connection_failed(server_type:ServerType, balancer_server_id:ServerID, error:sender::Error) -> Self {
        HandlerCommand::SenderCommand( SenderCommand::AcceptConnectionFailed(server_type, balancer_server_id, error) )
    }

    fn transaction_failed(server_type:ServerType, server_id:ServerID, error:sender::Error, old_basic_state:sender::BasicState) -> Self {
        HandlerCommand::SenderCommand( SenderCommand::TransactionFailed(server_type, server_id, error, old_basic_state) )
    }

    fn connected(server_type:ServerType, server_id:ServerID, balancer_server_id:ServerID, via_server_id:ServerID) -> Self {
        HandlerCommand::SenderCommand( SenderCommand::Connected(server_type, server_id, balancer_server_id, via_server_id) )
    }

    fn connected_to_all(server_type:ServerType) -> Self {
        HandlerCommand::SenderCommand( SenderCommand::ConnectedToAll(server_type) )
    }
}
