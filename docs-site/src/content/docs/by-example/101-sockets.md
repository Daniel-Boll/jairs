---
title: Sockets
description: A TCP client and server over AF_INET, a sockaddr_in passed by pointer, and the two-byte platform commitment that differs between macOS and Linux.
sidebar:
  order: 101
---

`Socket` wraps a descriptor exactly as `File` does and is deliberately a **different type**: a socket
accepts `send`/`recv`/`shutdown` and a file accepts `seek`, and the type is what stops a caller seeking a
socket (ADR-0158). Unlike `Process`, it works in both engines: `connect`, `bind` and `accept` take a
`sockaddr_in` **by pointer**, and a `Sockaddr_In` holds only integers, so the comptime VM's one-level
pointer translation is enough.

## A TCP socket, an address, and a connection

```jr
#import "Basic";
#import "File";

/// A socket.
Socket :: struct {
    /// The OS descriptor. Negative when the socket is not open.
    fd: s64;
}

/// A BSD `struct sockaddr_in`, exactly as macOS lays it out.
Sockaddr_In :: struct {
    /// `sin_len` — the structure's own size. BSD only; Linux has no such field.
    length: u8;
    /// `sin_family` — `AF_INET`. One byte on BSD, two on Linux.
    family: u8;
    /// `sin_port` — the port, in **network byte order**. Use `to_network_port`.
    port: u16;
    /// `sin_addr` — the IPv4 address, in network byte order. Use `parse_ipv4` or `ANY_ADDRESS`.
    address: u32;
    /// `sin_zero` — eight bytes that must be zero.
    zero_a: u32;
    /// The second half of `sin_zero`.
    zero_b: u32;
}

/// A TCP socket, unconnected and unbound.
///
/// `#must`: a caller holding a socket with a negative descriptor gets a failure from every later call, one
/// level removed from the cause.
make :: () -> (Socket, bool) #must { ... }

/// Connects `s` to `address` on `port`.
connect_to :: (s: *Socket, address: u32, port: s64) -> bool #must { ... }

/// Binds `s` to `port` on `address`, and starts listening with `backlog` pending connections.
///
/// One routine rather than `bind` then `listen`, because a bound socket that is not listening refuses
/// every connection, and splitting them is how a caller forgets the second half.
listen_on :: (s: *Socket, address: u32, port: s64, backlog: s64) -> bool #must { ... }

/// Accepts one connection. Blocks.
accept_one :: (s: *Socket) -> (Socket, bool) #must { ... }

/// The port `s` is bound to.
///
/// The routine that makes binding **port 0** usable: the OS chooses a free port and this reads it back,
/// which is how a test opens a server without picking a number that might be in use.
local_port :: (s: *Socket) -> (s64, bool) #must { ... }

/// Sends `bytes`, returning how many went and whether the call succeeded.
send :: (s: *Socket, bytes: []u8) -> (s64, bool) #must { ... }

/// Sends every byte of `bytes`.
send_all :: (s: *Socket, bytes: []u8) -> bool #must { ... }

/// Sends a string's bytes.
send_string :: (s: *Socket, text: string) -> bool #must { ... }

/// Receives up to `buffer.count` bytes.
///
/// **Zero means the peer closed**, which is a success and not an error — the same shape `File.read` has at
/// end of file, and the reason a caller loops until it sees zero rather than until a failure.
receive :: (s: *Socket, buffer: []u8) -> (s64, bool) #must { ... }

/// Stops one or both directions. `SHUT_WRITE` is how a caller says "I am done sending".
shutdown_socket :: (s: *Socket, how: s64) -> bool #must { ... }

/// Closes `s`. Returns the OS status, and is not `#must`.
close_socket :: (s: *Socket) -> s64 { ... }
```

`SOCKADDR_IN_SIZE :: 16;` and `layout_is_c_compatible()` exist because the Jairs `Sockaddr_In` struct
above must be exactly the sixteen bytes C's `sockaddr_in` is, and a drift would be wrong bytes on the wire
rather than a compile error. A file-scope constant can compute this just as well — `LAYOUT_OK ::
size_of(Sockaddr_In) == SOCKADDR_IN_SIZE;` checks and evaluates at file scope — but the module keeps the
check as a procedure the corpus program calls and checks.

## A client and a server talking to themselves

```jr
#import "Basic";
#import "Socket";

main :: () {
    total := 0;

    if layout_is_c_compatible() {
        total = total + 1;
    }

    loopback, loopback_ok := parse_ipv4("127.0.0.1");
    if loopback_ok {
        if loopback == cast(u32, LOOPBACK_ADDRESS) {
            total = total + 2;
        }
    }

    server, server_ok := make();
    if !server_ok {
        exit(1);
    }
    if set_reuse_address(*server) {
        total = total + 16;
    }
    if listen_on(*server, cast(u32, ANY_ADDRESS), 0, 4) {
        total = total + 32;
    }
    port, port_ok := local_port(*server);
    if port_ok {
        if port > 0 {
            total = total + 64;
        }
    }

    client, client_ok := make();
    if !client_ok {
        exit(2);
    }
    if connect_to(*client, cast(u32, LOOPBACK_ADDRESS), port) {
        total = total + 128;
    }

    peer, peer_ok := accept_one(*server);
    if peer_ok {
        if send_string(*client, "ping") {
            total = total + 256;
        }
        buffer: [16]u8;
        got, got_ok := receive(*peer, view(*buffer[0], 16));
        if got_ok {
            if got == 4 {
                total = total + 512;
            }
        }
        if send_string(*peer, "pong!") {
            total = total + 2048;
        }
        if shutdown_socket(*peer, SHUT_WRITE) {
            total = total + 4096;
        }
        answer, answer_ok := receive(*client, view(*buffer[0], 16));
        if answer_ok {
            if answer == 5 {
                total = total + 8192;
            }
        }
        // Zero is the peer closing, and it is a **success**. A caller who treated it as failure would loop
        // forever on a healthy connection, which is why `receive`'s contract says so.
        ended, ended_ok := receive(*client, view(*buffer[0], 16));
        if ended_ok {
            if ended == 0 {
                total = total + 16384;
            }
        }
        _ = close_socket(*peer);
    }

    _ = close_socket(*client);
    _ = close_socket(*server);
    exit(total % 251);
}
```

This program talks to itself over the loopback interface — unusual for a corpus file, and the point: a
socket module whose test never opened a connection would only be checking its byte-order arithmetic. It
binds **port 0** so the OS picks a free port, which is exactly what `local_port` exists to read back, and
what stops the suite failing on a busy machine. Client and server are one process, in the order listen,
connect, accept: that works with no concurrency because `connect` to a listening socket completes into
the kernel's backlog before anyone calls `accept`. Run alone, this excerpt exits **105**. The full corpus
file — `tests/corpus/valid/129-sockets.jr` — adds four address-parsing refusals, a byte check on the
received data (`buffer[0] == 112`), and a port byte-swap round-trip, and exits **137**.

## The platform commitment, and why it is bigger here than for `File`

```jr
/// `AF_INET` — IPv4. 2 on macOS and Linux.
AF_INET :: 2;

/// `SOL_SOCKET`, for `set_reuse_address`. **0xffff on macOS**, 1 on Linux.
SOL_SOCKET :: #run sol_socket_for(os());

/// `SOL_SOCKET` for `target` — **0xffff on macOS and 1 on Linux**.
sol_socket_for :: (target: Operating_System) -> s64 {
    if target == Operating_System.MACOS {
        return 65535;
    }
    return 1;
}
```

The `Sockaddr_In` layout below is macOS's, and it differs from Linux's in the first two bytes: BSD lays
out `{ uint8_t sin_len; sa_family_t sin_family; }` where Linux has a single `{ uint16_t sin_family; }`.
The *size* agrees — sixteen bytes either way — and only those two bytes disagree, which is why no
per-OS struct is generated; a value difference needs no `#insert`. A Linux build needs `length` removed
and `family` widened to sixteen bits, and until that lands, this module has only ever run on macOS.

`to_network_port` swaps byte order by hand rather than binding `htons`, for two reasons: `htons` is a
*macro* on some platforms, so there may be no symbol to bind at all — the same reason `Process` decodes
`waitpid`'s status itself rather than calling a macro — and the swap is two arithmetic operations, cheaper
than a foreign call. It assumes a little-endian host, which both arm64 and x86-64 are, and says so.

## What is absent, and why

`Socket` has <span class="jairs-status absent">absent</span> name resolution: `getaddrinfo` returns a
linked list of `struct addrinfo` — pointers inside pointers — which is exactly the shape neither the
`#foreign` aggregate refusal (E0286) nor the VM's one-level pointer translation can handle. A caller who
needs DNS today runs `dig` through `Process` and parses the answer.

There is also <span class="jairs-status absent">absent</span> IPv6 (`sockaddr_in6` needs a second address
type or a union — a real design question, and the wrong one to answer while the aggregate boundary is
still closed), <span class="jairs-status absent">absent</span> non-blocking mode and `select` (both exist
to wait on several descriptors at once, which is concurrency, and `select`'s `fd_set` is itself an
aggregate), and <span class="jairs-status absent">absent</span> TLS — not a socket feature at all, but a
protocol layered over one, needing primitives (large-integer arithmetic, hashes, constant-time comparison)
none of which exist in this library.

See also [Book I — The Jairs Language](/language/introduction/).
