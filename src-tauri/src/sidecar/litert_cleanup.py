"""Ascuta cleanup sidecar: runs Gemma through LiteRT-LM (the same runtime and
`.litertlm` model the Android app uses) and answers JSON-line requests.

stdin : {"id": 1, "system": "...", "text": "...", "max_tokens": 256}
stdout: {"ready": true}  then  {"id": 1, "text": "..."} | {"id": 1, "error": "..."}
Native library noise is redirected to stderr so stdout stays a clean protocol.
"""
import json
import os
import sys

# Keep a private copy of the real stdout for the protocol, then point fd 1 at
# stderr so C++ logging cannot corrupt it.
_proto = os.fdopen(os.dup(1), "w", buffering=1)
os.dup2(2, 1)


def send(obj):
    _proto.write(json.dumps(obj, ensure_ascii=False) + "\n")
    _proto.flush()


def main():
    import litert_lm

    litert_lm.set_min_log_severity(litert_lm.LogSeverity.ERROR)
    model, threads = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else None
    backend = litert_lm.Backend.CPU(thread_count=threads)
    with litert_lm.Engine(model, backend=backend) as engine:
        send({"ready": True})
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                req = json.loads(line)
                # Same sampling as the Android app: greedy-ish, short answer.
                with engine.create_conversation(
                    system_message=req["system"],
                    sampler_config=litert_lm.SamplerConfig(
                        top_k=1, top_p=1.0, temperature=0.1
                    ),
                    max_output_tokens=int(req.get("max_tokens", 256)),
                ) as conversation:
                    resp = conversation.send_message(req["text"])
                    text = resp["content"][0]["text"]
                send({"id": req.get("id"), "text": text})
            except Exception as exc:  # keep serving after a bad request
                send({"id": None, "error": f"{type(exc).__name__}: {exc}"})


if __name__ == "__main__":
    main()
