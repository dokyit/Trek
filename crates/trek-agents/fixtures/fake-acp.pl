#!/usr/bin/perl
# A stand-in ACP agent for tests: answers initialize, session/new, session/load and
# session/prompt (saying how many text blocks it got), and logs every message it's sent to
# acp-log.jsonl in its working folder. A prompt "hold" isn't answered until the next prompt, a
# "refuse" that comes while one is held is turned down, and the held one then ends.
use strict;
use warnings;
use JSON::PP;

$| = 1;
my $json = JSON::PP->new->canonical;
open(my $log, '>>', 'acp-log.jsonl') or die "log: $!";
select((select($log), $| = 1)[0]);
my $sessions = 0;
my $held;
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
        my @texts = grep { $_->{type} eq 'text' } @{ $m->{params}{prompt} };
        my $text = $texts[0]{text} // '';
        if ($text eq 'hold') {
            $held = $m->{id};
            next;
        }
        if ($text eq 'refuse' && defined $held) {
            print $json->encode({ jsonrpc => '2.0', id => $m->{id}, error => { code => -32602, message => 'a prompt is already running' } }), "\n";
            print $json->encode({ jsonrpc => '2.0', id => $held, result => { stopReason => 'end_turn' } }), "\n";
            undef $held;
            next;
        }
        my $texts = @texts;
        my $update = { sessionUpdate => 'agent_message_chunk', content => { type => 'text', text => "heard $texts" } };
        print $json->encode({ jsonrpc => '2.0', method => 'session/update', params => { sessionId => $m->{params}{sessionId}, update => $update } }), "\n";
        $result = { stopReason => 'end_turn' };
    }
    print $json->encode({ jsonrpc => '2.0', id => $m->{id}, result => $result }), "\n";
}
