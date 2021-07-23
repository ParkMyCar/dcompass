// Copyright 2020 LEXUGE
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use droute::{actions::CacheMode, builders::*, mock::Server, AsyncTryInto};
use once_cell::sync::Lazy;
use tokio::net::UdpSocket;
use trust_dns_proto::{
    op::{header::MessageType, query::Query, Message, OpCode, ResponseCode},
    rr::{record_data::RData, record_type::RecordType, resource::Record, Name},
};

static DUMMY_MSG: Lazy<Message> = Lazy::new(|| {
    let mut msg = Message::new();
    msg.add_answer(Record::from_rdata(
        Name::from_utf8("www.apple.com").unwrap(),
        32,
        RData::A("1.1.1.1".parse().unwrap()),
    ));
    msg.set_message_type(MessageType::Response);
    msg
});

static QUERY: Lazy<Message> = Lazy::new(|| {
    let mut msg = Message::new();
    msg.add_query(Query::query(
        Name::from_utf8("www.apple.com").unwrap(),
        RecordType::A,
    ));
    msg.set_message_type(MessageType::Query);
    msg
});

#[tokio::test]
async fn test_resolve() {
    let socket = UdpSocket::bind(&"127.0.0.1:53533").await.unwrap();
    let server = Server::new(socket, vec![0; 1024], None);
    tokio::spawn(server.run(DUMMY_MSG.clone()));

    let router = RouterBuilder::new(
        TableBuilder::new().add_rule(
            "start",
            RuleBuilders::IfBlock(IfBlockBuilder {
                matcher: BuiltinMatcherBuilders::Any,
                on_match: BranchBuilder::new("end").add_action(BuiltinActionBuilders::Query(
                    QueryBuilder::new("mock", CacheMode::default()),
                )),
                no_match: BranchBuilder::default(),
            }),
        ),
        UpstreamsBuilder::new(1).unwrap().add_upstream(
            "mock",
            UdpBuilder {
                addr: "127.0.0.1:53533".parse().unwrap(),
                dnssec: false,
                timeout: 10,
            },
        ),
    )
    .try_into()
    .await
    .unwrap();

    assert_eq!(
        router.resolve(QUERY.clone()).await.unwrap().answers(),
        DUMMY_MSG.answers()
    );

    // Shall not accept messages with no queries.
    assert_eq!(
        router.resolve(Message::new()).await.unwrap(),
        Message::error_msg(0, OpCode::Query, ResponseCode::ServFail)
    );
}
