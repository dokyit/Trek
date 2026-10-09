#!/usr/bin/perl
# A stand-in ACP agent for tests: answers initialize, session/new, session/load and
# session/prompt (saying how many text blocks it got), and logs every message it's sent to
# acp-log.jsonl in its working folder. A prompt "hold" isn't answered until the next prompt, a
# "refuse" that comes while one is held is turned down, and the held one then ends.
# session/new and session/load turn down `mcpServers` that aren't what the spec says: an array
# of stdio servers {name, command, args, env: [{name, value}]}, plus {type: "http", name, url,
# headers: [{name, value}]} only when the agent said it takes HTTP. It says so when its working
# folder has a fake-acp-mcp.json, which becomes its `mcpCapabilities` (e.g. {"http": true}).
use strict;
use warnings;
use JSON::PP;

$| = 1;
my $json = JSON::PP->new->canonical;
open(my $log, '>>', 'acp-log.jsonl') or die "log: $!";
select((select($log), $| = 1)[0]);
my $sessions = 0;
my $held;
my $mcp_caps = {};
if (open(my $caps, '<', 'fake-acp-mcp.json')) {
    local $/;
    $mcp_caps = $json->decode(<$caps>);
}

# Why `servers` isn't a valid `mcpServers`, or undef.
sub bad_mcp {
    my ($servers) = @_;
    return 'mcpServers must be an array' unless ref $servers eq 'ARRAY';
    my $pairs = sub {
        my ($list) = @_;
        return 0 unless ref $list eq 'ARRAY';
        for (@$list) { return 0 unless ref $_ eq 'HASH' && defined $_->{name} && !ref $_->{name} && defined $_->{value} && !ref $_->{value} }
        return 1;
    };
    for my $s (@$servers) {
        return 'a server must be an object' unless ref $s eq 'HASH';
        return 'a server needs a name' unless defined $s->{name} && !ref $s->{name};
        my $type = $s->{type} // 'stdio';
        if ($type eq 'stdio') {
            return "$s->{name}: stdio needs command, args and env" unless defined $s->{command} && !ref $s->{command} && ref $s->{args} eq 'ARRAY' && $pairs->($s->{env});
        } elsif ($type eq 'http' || $type eq 'sse') {
            return "$s->{name}: $type isn't supported" unless $mcp_caps->{$type};
            return "$s->{name}: $type needs url and headers" unless defined $s->{url} && !ref $s->{url} && $pairs->($s->{headers});
        } else {
            return "$s->{name}: unknown type $type";
        }
    }
    return undef;
}
while (my $line = <STDIN>) {
    my $m = eval { $json->decode($line) } or next;
    next unless defined $m->{method};
    print $log $json->encode({ method => $m->{method}, params => $m->{params} }), "\n";
    next unless defined $m->{id};
    my $method = $m->{method};
    my $result = {};
    if ($method eq 'initialize') {
        $result = { protocolVersion => 1, agentCapabilities => { loadSession => JSON::PP::true, mcpCapabilities => $mcp_caps } };
    } elsif (($method eq 'session/new' || $method eq 'session/load') && defined(my $why = bad_mcp($m->{params}{mcpServers}))) {
        print $json->encode({ jsonrpc => '2.0', id => $m->{id}, error => { code => -32602, message => "Invalid params: $why" } }), "\n";
        next;
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
