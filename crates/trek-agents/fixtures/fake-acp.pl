#!/usr/bin/perl
# A stand-in ACP agent for tests: answers initialize, session/new, session/load and
# session/prompt (saying how many text blocks it got), and logs every message it's sent to
# acp-log.jsonl in its working folder.
use strict;
use warnings;
use JSON::PP;

$| = 1;
my $json = JSON::PP->new->canonical;
open(my $log, '>>', 'acp-log.jsonl') or die "log: $!";
select((select($log), $| = 1)[0]);
my $sessions = 0;
while (my $line = <STDIN>) {
    my $m = eval { $json->decode($line) } or next;
    next unless defined $m->{method};
    print $log $json->encode({ method => $m->{method}, params => $m->{params} }), "\n";
    next unless defined $m->{id};
    my $method = $m->{method};
    my $result = {};
    if ($method eq 'initialize') {
        $result = { protocolVersion => 1, agentCapabilities => { loadSession => JSON::PP::true } };
    } elsif ($method eq 'session/new') {
        $sessions++;
        $result = { sessionId => "fake-$sessions" };
    } elsif ($method eq 'session/prompt') {
        my $texts = grep { $_->{type} eq 'text' } @{ $m->{params}{prompt} };
        my $update = { sessionUpdate => 'agent_message_chunk', content => { type => 'text', text => "heard $texts" } };
        print $json->encode({ jsonrpc => '2.0', method => 'session/update', params => { sessionId => $m->{params}{sessionId}, update => $update } }), "\n";
        $result = { stopReason => 'end_turn' };
    }
    print $json->encode({ jsonrpc => '2.0', id => $m->{id}, result => $result }), "\n";
}
