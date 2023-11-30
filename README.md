# a shared mem(e)ory queue

This is an experimental library for fast IPC on Linux. On my preliminary benchmarks, it’s much faster and more consistent than passing messages via Unix-domain sockets.
You can try running benchmarks yourself, they’re in `benchmarks/` directory.

It’s very much WIP, eventfd synchronization is totally broken and there’s no async support yet. If you want to be notified on the first proper release, please subscribe to GitHub releases by clicking Watch -> Custom -> Releases.
