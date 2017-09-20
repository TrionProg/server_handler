//!Представляет собой конечный автомат, содержащий состояние всей игры.

use std;
use handler;
use sender;
use nes::{ErrorInfo,ErrorInfoTrait};

use std::sync::{Arc,Mutex,RwLock};

use common_messages::MessageServerID;
use common_messages::HandlerToBalancer;

use handler::HandlerSender;

use ::ThreadSource;
use ::ArcSender;
use ::ArcProperties;
use ::{HandlerCommand};

use sender::SenderTrait;

pub type ArcAutomat=Arc<Automat>;

///Конечный Автомат
pub struct Automat {
    properties:ArcProperties,
    sender:ArcSender,
//    sender:ArcSender,
    state:Mutex<State>,
    handler_sender:Mutex<HandlerSender>,
}

///Состояния Автомата
#[derive(Copy,Clone,Eq,PartialEq,Debug)]
pub enum State {
    ///Инициализация
    Initialization,
    ///Знакомимся с другими серверами
    Familiarity(usize),
    ///Рабочее состояние
    Working,
    ///Сервер остановлен
    Shutdown
}

///TransactionError - Ошибка транзакции
///Poisoned:Mutex сломан(FatalError)
///BrockenChannel:Канал HandlerSender сломан(FatalError)
///SenderTransactionFailed:Транзакция сендера проведена не успешно, послано сообщение <sender>Command::TransactionFailed

define_error!( TransactionError,
    Poisoned() =>
        "Poisoned",
    BrockenChannel() =>
        "Channel for Handler is broken",
    BalancerCrash(sender_error:Box<sender::Error>, thread_source:ThreadSource) =>
        "[Source:{2}] Balancer server has crashed: {1}",
    ServerTransactionFailed() =>
        "Server transaction has failed"
);

macro_rules! do_sender_transaction {
    [$handler_sender:expr,$operation:expr] => {
        match $operation {
            Ok(_) => {},
            Err(::sender::TransactionError::Poisoned(error_info)) => return Err( TransactionError::Poisoned(error_info) ),
            Err(::sender::TransactionError::BrockenChannel(error_info)) => return Err( TransactionError::BrockenChannel(error_info) ),
            Err(::sender::TransactionError::TransactionFailed(error_info)) => {//TODO пока хз что делать
                let handler_sender=mutex_lock!(&$handler_sender,TransactionError);
                //channel_send!(handler_sender,HandlerCommand::Error,TransactionError);
                return Err( TransactionError::ServerTransactionFailed(error_info) );
            },
            Err(::sender::TransactionError::NoServer(..)) => unreachable!(),
            Err(::sender::TransactionError::NoServerWithUniqueID(..)) => unreachable!(),
        }
    };
}

impl Automat {
    ///Создаёт Автомат
    pub fn new(sender:ArcSender, properties:ArcProperties, handler_sender:HandlerSender) -> Self {
        Automat {
            properties:properties,
            sender:sender,
            state:Mutex::new(State::Initialization),
            handler_sender:Mutex::new(handler_sender),
        }
    }

    ///Создаёт ArcAutomat или Arc<Automat>
    pub fn new_arc(sender:ArcSender, properties:ArcProperties, handler_sender:HandlerSender) -> ArcAutomat {
        Arc::new( Automat::new(sender, properties, handler_sender) )
    }

    ///Эта функция запускается первой и отвечает Balancer-у
    pub fn answer_to_balancer(&self) -> result![TransactionError]{
        try!(self.sender.balancer_sender.send(&HandlerToBalancer::ServerStarted), TransactionError::BalancerCrash, ThreadSource::Handler);

        ok!()
    }

    ///Выполняет знакомство, если сервер один, то сразу переключает состояние в Working, если несколько, то знакомится.
    ///При переключении в состояние Working, устанавливает состояние и отправляет handlerCommand::FamiliarityFinished
    pub fn familiarize(&self,
        storages:&Vec<(MessageServerID,String)>,
        handlers:&Vec<(MessageServerID,String)>
    ) -> result![TransactionError]{
        let mut state_guard=mutex_lock!(&self.state,TransactionError);

        let count=(if storages.len()>0 {1} else {0}) + (if handlers.len()>0 {1} else {0});

        if count==0 {
            *state_guard=State::Working;
            try!(self.sender.balancer_sender.send(&HandlerToBalancer::FamiliarityFinished), TransactionError::BalancerCrash, ThreadSource::Handler);
        }else{
            *state_guard=State::Familiarity(count);
            do_sender_transaction![&self.handler_sender, self.sender.familiarize(storages, handlers)];
        }

        ok!()
    }

    ///Вызывается, когда Handler познакомился с серверами определённого типа, уменьшаем количество серверов(типов), с которыми надо познакомиться,
    ///Если это число становится равным 0, то переключаемся в состояние Working, при этом отправляется handlerCommand::FamiliarityFinished
    pub fn connected_to_all(&self) -> result![TransactionError] {
        let mut state_guard=mutex_lock!(&self.state,TransactionError);

        let mut next_state=match *state_guard {
            State::Familiarity(ref mut count) => {
                *count-=1;

                *count==0
            },
            _ => false//TODO other states
        };

        if next_state {
            info!("Working");
            *state_guard=State::Working;
            try!(self.sender.balancer_sender.send(&HandlerToBalancer::FamiliarityFinished), TransactionError::BalancerCrash, ThreadSource::Handler);
        }

        ok!()
    }

    ///Переводит сервер в shutdown стадию, если он уже находится в этой стадии, ничего не происходит
    pub fn shutdown(&self) -> result![TransactionError] {
        let mut state_guard=mutex_lock!(&self.state,TransactionError);

        match state_guard.clone() {
            State::Initialization => *state_guard=State::Shutdown,
            State::Familiarity(_) => {
                *state_guard=State::Shutdown;

                //TODO попрощаться с серверами
                //do_sender_transaction![self.handler_sender, self.sender.handlers.shutdown_all()];
            },
            State::Working => {
                *state_guard=State::Shutdown;

                //TODO попрощаться с серверами

                //do_sender_transaction![self.handler_sender, self.sender.handlers.shutdown_all()];
            },
            _ => {}
        }

        ok!()
    }
}
