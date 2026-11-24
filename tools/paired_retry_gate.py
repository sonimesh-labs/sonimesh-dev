"""One-shot loopback gate for the Android paired-connection retry test."""

import select
import socket


def main() -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as signal:
        signal.bind(("127.0.0.1", 47611))
        signal.settimeout(60)
        print("Waiting for the emulator retry signal", flush=True)
        signal.recvfrom(10)

    with socket.socket() as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", 47610))
        listener.listen(1)
        listener.settimeout(30)
        print("Retry gate open", flush=True)
        client, _ = listener.accept()
        with client, socket.create_connection(("127.0.0.2", 47610), timeout=5) as target:
            target.settimeout(None)
            while True:
                ready, _, _ = select.select([client, target], [], [], 30)
                if not ready:
                    return
                for source in ready:
                    data = source.recv(16_384)
                    if not data:
                        return
                    (target if source is client else client).sendall(data)


if __name__ == "__main__":
    main()
