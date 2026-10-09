#!/usr/bin/perl
# A stand-in MCP server (stdio) for tests. It logs a line to stdout before the protocol starts
# (as some real servers do), asks the client to `ping` before answering initialize, and lists
# its tools in two pages. Its first argument picks how it behaves:
#   ok      answer normally; a tool `env_ok` is listed when FAKE_TOKEN is "abc"
#   crash   print an error to stderr and exit before answering
#   silent  read everything and never answer
#   refuse  answer initialize with a JSON-RPC error
use strict;
use warnings;
use JSON::PP;

$| = 1;
my $json = JSON::PP->new->canonical;
my $mode = shift // 'ok';

if ($mode eq 'crash') {
    print STDERR "npm warn exec The following package was not found and will be installed\n";
    print STDERR "Error: GITHUB_PERSONAL_ACCESS_TOKEN environment variable is required\n";
    exit 1;
}
print "fake-mcp starting up\n";

sub send_msg { print $json->encode($_[0]), "\n" }

while (my $line = <STDIN>) {
    chomp $line;
    next if $line eq '';
    my $msg = eval { $json->decode($line) } or next;
    next if $mode eq 'silent';
    my $method = $msg->{method} // '';
    my $id = $msg->{id};
    if ($method eq 'initialize') {
        if ($mode eq 'refuse') {
            send_msg({ jsonrpc => '2.0', id => $id, error => { code => -32602, message => 'Unsupported protocol version' } });
            next;
        }
        # A request of the server's own first; the client must answer it and carry on.
        send_msg({ jsonrpc => '2.0', id => 'srv-1', method => 'ping' });
        my $reply = <STDIN>;
        my $r = eval { $json->decode($reply) } || {};
        die "no ping reply" unless defined $r->{id} && $r->{id} eq 'srv-1' && $r->{result};
        send_msg({ jsonrpc => '2.0', id => $id, result => { protocolVersion => $msg->{params}{protocolVersion}, capabilities => { tools => {} }, serverInfo => { name => 'fake-mcp', version => '1.0' } } });
    } elsif ($method eq 'notifications/initialized') {
        # Nothing to say.
    } elsif ($method eq 'tools/list') {
        my $cursor = $msg->{params}{cursor};
        if (!defined $cursor) {
            send_msg({ jsonrpc => '2.0', method => 'notifications/message', params => { level => 'info', data => 'listing' } });
            send_msg({ jsonrpc => '2.0', id => $id, result => { tools => [{ name => 'echo' }, { name => 'add' }], nextCursor => 'page2' } });
        } else {
            my @tools = ({ name => 'get_time' });
            push @tools, { name => 'env_ok' } if ($ENV{FAKE_TOKEN} // '') eq 'abc';
            send_msg({ jsonrpc => '2.0', id => $id, result => { tools => \@tools } });
        }
    } elsif (defined $id) {
        send_msg({ jsonrpc => '2.0', id => $id, error => { code => -32601, message => 'Method not found' } });
    }
}
