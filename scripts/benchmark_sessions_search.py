#!/usr/bin/env python3
"""Read-only local-corpus search experiment. Prints aggregate metrics, never text.

Temporary databases live in a mode-0700 TemporaryDirectory and are removed.
Cold means a fresh SQLite connection, not a flushed OS page cache. Fixtures sample
observed message lengths with a fixed seed; no private text enters fixture output.
"""
import argparse
import json
import pathlib
import random
import resource
import sqlite3
import statistics
import tempfile
import time


def evidence(message):
    if not isinstance(message, dict):
        return ""
    role, body = next(iter(message.items()))
    if role == "ToolResult":
        return body.get("content", "")
    if role in ("EnrichedAssistant", "AbortedAssistant"):
        body = body.get("content", [])
    if not isinstance(body, list):
        return ""
    parts = []
    for block in body:
        if "Text" in block:
            parts.append(block["Text"])
        elif "ToolCall" in block:
            call = block["ToolCall"]
            parts.append(call["name"] + " " + json.dumps(call["arguments"]))
    return "\n".join(parts)


def corpus(root):
    docs, sizes, malformed = [], [], 0
    paths = list(root.glob("*/*/session.jsonl")) + list(root.glob("*/*.jsonl"))
    for path in sorted(paths):
        sizes.append(path.stat().st_size)
        with path.open() as source:
            for line in source:
                try:
                    record = json.loads(line)
                except (ValueError, UnicodeError):
                    malformed += 1
                    continue
                messages = ([record.get("display_message") or record["message"]] if record.get("type") == "message" else
                            [item["message"] for item in record.get("display_messages", [])])
                docs.extend(text for text in map(evidence, messages) if text)
    return docs, sizes, malformed


def percentile(values, fraction):
    return sorted(values)[min(len(values) - 1, int(len(values) * fraction))]


def experiment(docs, directory, label):
    queries = ["compaction", "session index", "E0308", "cargo test", "workspace", "cancellation", "zzabsentneedlezz"]
    results = {}
    for engine, tokenizer in [("scan", None), ("unicode61", "unicode61"), ("porter", "porter unicode61"), ("trigram", "trigram")]:
        path = pathlib.Path(directory) / (label + engine + ".sqlite")
        connection = sqlite3.connect(path)
        start = time.perf_counter()
        cpu_start = time.process_time()
        connection.execute("create table docs(text)" if tokenizer is None else
                           f"create virtual table docs using fts5(text, tokenize='{tokenizer}')")
        connection.executemany("insert into docs values (?)", ((doc,) for doc in docs))
        connection.commit()
        build = time.perf_counter() - start
        build_cpu = time.process_time() - cpu_start
        latencies, cold, hits = [], [], []
        for query in queries:
            terms = query.split()
            if tokenizer is None:
                sql = "select rowid from docs where " + " and ".join("instr(lower(text),?)>0" for _ in terms) + " limit 10"
                args = [term.lower() for term in terms]
            else:
                sql = "select rowid from docs where docs match ? order by rank limit 10"
                args = [' AND '.join('"' + t + '"' for t in terms)]
            connection.close()
            connection = sqlite3.connect(path)
            start = time.perf_counter()
            found = connection.execute(sql, args).fetchall()
            cold.append((time.perf_counter() - start) * 1000)
            hits.append(len(found))
            for _ in range(10):
                start = time.perf_counter()
                connection.execute(sql, args).fetchall()
                latencies.append((time.perf_counter() - start) * 1000)
        start = time.perf_counter()
        connection.execute("insert into docs values (?)", ("incremental new evidence E0308",))
        connection.commit()
        append_ms = (time.perf_counter() - start) * 1000
        connection.close()
        results[engine] = dict(build_s=round(build, 3), build_cpu_s=round(build_cpu, 3), size_bytes=path.stat().st_size,
                               cold_p50_ms=round(statistics.median(cold), 3), cold_p95_ms=round(percentile(cold, .95), 3),
                               warm_p50_ms=round(statistics.median(latencies), 3), warm_p95_ms=round(percentile(latencies, .95), 3),
                               append_ms=round(append_ms, 3), hit_counts=hits)
    return results


def quality():
    # Labeled retrieval tasks: lexical, stemming, identifier, substring, absence.
    docs = ["fix cancellation during session compaction", "cancelled requests must restore terminal", "E0308 mismatched types in SessionStore", "workspace checkpoint restore", "noise " * 100 + "session compaction"]
    cases = [("session compaction", {1}), ("cancel", {1, 2}), ("E0308", {3}), ("SessionStore", {3}), ("checkpoint", {4}), ("point", {4}), ("unfindable", set())]
    scores = {}
    for engine, tokenizer in [("scan", None), ("unicode61", "unicode61"), ("porter", "porter unicode61"), ("trigram", "trigram")]:
        connection = sqlite3.connect(":memory:")
        connection.execute("create table docs(text)" if tokenizer is None else f"create virtual table docs using fts5(text,tokenize='{tokenizer}')")
        connection.executemany("insert into docs values (?)", ((x,) for x in docs))
        reciprocal, recall = [], []
        for query, relevant in cases:
            terms = query.split()
            if tokenizer is None:
                rows = connection.execute("select rowid from docs where " + " and ".join("instr(lower(text),?)>0" for _ in terms) + " limit 3", [t.lower() for t in terms]).fetchall()
            else:
                rows = connection.execute("select rowid from docs where docs match ? order by rank limit 3", (' AND '.join('"'+t+'"' for t in terms),)).fetchall()
            ids = [r[0] for r in rows]
            if relevant:
                reciprocal.append(next((1/(i+1) for i,r in enumerate(ids) if r in relevant),0))
                recall.append(len(set(ids) & relevant)/len(relevant))
        scores[engine] = dict(mrr_at_3=round(statistics.mean(reciprocal),3), recall_at_3=round(statistics.mean(recall),3))
        connection.close()
    return scores


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path.home()/".rho/sessions")
    parser.add_argument("--scale", type=int, default=3)
    args = parser.parse_args()
    start = time.perf_counter()
    docs, sizes, malformed = corpus(args.root)
    if not docs:
        raise SystemExit("no display evidence found")
    lengths = [len(x) for x in docs]
    output = dict(sqlite=sqlite3.sqlite_version, sessions=len(sizes), raw_bytes=sum(sizes), evidence_messages=len(docs),
                  evidence_bytes=sum(len(x.encode()) for x in docs), message_chars_p50=statistics.median(lengths),
                  message_chars_p95=percentile(lengths,.95), malformed_records=malformed, extraction_s=round(time.perf_counter()-start,3))
    with tempfile.TemporaryDirectory(prefix="rho-search-bench-") as directory:
        output["actual"] = experiment(docs, directory, "actual")
        randomizer = random.Random(7)
        vocabulary = ["compaction", "session", "index", "E0308", "cargo", "test", "workspace", "cancellation"] + [f"word{i}" for i in range(512)]
        fixtures = [" ".join(randomizer.choices(vocabulary,k=max(1, randomizer.choice(lengths)//8))) for _ in range(len(docs)*args.scale)]
        output["scaled_messages"] = len(fixtures)
        output["scaled"] = experiment(fixtures, directory, "scaled")
        output["quality_tiny_labeled_fixture"] = quality()
    output["peak_rss_kib_linux"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    print(json.dumps(output, indent=2))


if __name__ == "__main__":
    main()
