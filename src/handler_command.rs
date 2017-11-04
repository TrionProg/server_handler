use ipc_listener;
use sender;

use common_messages::MessageConnectionID;
use ::{ServerType,ServerID,ConnectionID};
use ::ThreadSource;

pub enum HandlerCommand {
    IpcListenerThreadCrash(ThreadSource),
    BalancerCrash(ThreadSource),

    IpcListenerSetupError,
    IpcListenerIsReady,
    ShutdownReceived,
    Shutdown,
    IpcListenerFinished,
    Task,

    //From IPC Listener
    Familiarity(Box<(Vec<(ServerID,MessageConnectionID,String)>,Vec<(ServerID,MessageConnectionID,String)>,usize)>),
    AcceptConnection(ServerType,ServerID,ConnectionID,String,ConnectionID),
    ConnectionAccepted(ServerType,ConnectionID,ConnectionID),
    Connected(ServerType,ConnectionID),

    SenderCommand(SenderCommand),
}

//From Sender
pub enum SenderCommand {
    ConnectionFailed(ServerType, ConnectionID, sender::Error),
    AcceptConnectionFailed(ServerType, ConnectionID, sender::Error),
    TransactionFailed(ServerType, ConnectionID, sender::Error, sender::BasicState),
    Connected(ServerType, ConnectionID, ConnectionID, ConnectionID),
    ConnectedToAll(ServerType)
}

impl sender::SenderCommand for HandlerCommand{
    fn connection_failed(server_type:ServerType, balancer_connection_id:ConnectionID, error:sender::Error) -> Self {
        HandlerCommand::SenderCommand( SenderCommand::ConnectionFailed(server_type, balancer_connection_id, error) )
    }

    fn accept_connection_failed(server_type:ServerType, balancer_connection_id:ConnectionID, error:sender::Error) -> Self {
        HandlerCommand::SenderCommand( SenderCommand::AcceptConnectionFailed(server_type, balancer_connection_id, error) )
    }

    fn transaction_failed(server_type:ServerType, connection_id:ConnectionID, error:sender::Error, old_basic_state:sender::BasicState) -> Self {
        HandlerCommand::SenderCommand( SenderCommand::TransactionFailed(server_type, connection_id, error, old_basic_state) )
    }

    fn connected(server_type:ServerType, connection_id:ConnectionID, balancer_connection_id:ConnectionID, via_connection_id:ConnectionID) -> Self {
        HandlerCommand::SenderCommand( SenderCommand::Connected(server_type, connection_id, balancer_connection_id, via_connection_id) )
    }

    fn connected_to_all(server_type:ServerType) -> Self {
        HandlerCommand::SenderCommand( SenderCommand::ConnectedToAll(server_type) )
    }
}
