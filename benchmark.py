#!/usr/bin/env python3
"""Benchmark Go vs Rust versions of cairn-code."""

import subprocess
import time
import os
import sys
import statistics
import tempfile
import json
from pathlib import Path

BENCH_DIR = Path(tempfile.gettempdir()) / "cairn-bench"
GO_BIN = BENCH_DIR / "cairn-go.exe"
RUST_BIN = BENCH_DIR / "cairn-rust.exe"
CONFIG_PATH = Path.home() / ".config" / "cairn-code" / "config.json"

def get_api_key():
    """Read OpenRouter API key from config file or environment."""
    # Check environment first
    key = os.environ.get("OPENROUTER_API_KEY")
    if key:
        return key
    # Check config file
    if CONFIG_PATH.exists():
        try:
            with open(CONFIG_PATH) as f:
                cfg = json.load(f)
            return cfg.get("api_keys", {}).get("openrouter")
        except:
            pass
    return None

def check_binaries():
    """Verify both binaries exist."""
    if not GO_BIN.exists():
        print(f"ERROR: Go binary not found at {GO_BIN}")
        sys.exit(1)
    if not RUST_BIN.exists():
        print(f"ERROR: Rust binary not found at {RUST_BIN}")
        sys.exit(1)
    print(f"Go binary:   {GO_BIN} ({GO_BIN.stat().st_size / 1024 / 1024:.2f} MB)")
    print(f"Rust binary: {RUST_BIN} ({RUST_BIN.stat().st_size / 1024 / 1024:.2f} MB)")
    print()

def measure_startup(name, binary, runs=5):
    """Measure time from process start to first output."""
    times = []
    for i in range(runs):
        env = os.environ.copy()
        env["TERM"] = "xterm-256color"
        start = time.perf_counter()
        try:
            proc = subprocess.Popen(
                [str(binary), "-p", "say hello"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=env,
                creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0,
            )
            proc.wait(timeout=10)
            elapsed = time.perf_counter() - start
            times.append(elapsed)
        except subprocess.TimeoutExpired:
            proc.kill()
            times.append(10.0)
        except Exception as e:
            times.append(10.0)
    return times

def measure_memory(name, binary):
    """Measure peak memory usage."""
    env = os.environ.copy()
    env["TERM"] = "xterm-256color"
    try:
        proc = subprocess.Popen(
            [str(binary), "-p", "hello"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0,
        )
        proc.wait(timeout=10)
    except:
        pass
    return 0  # Windows memory measurement requires psutil

def measure_idle_cpu(name, binary, duration=2):
    """Measure CPU usage during idle (just process creation overhead)."""
    env = os.environ.copy()
    env["TERM"] = "xterm-256color"
    try:
        proc = subprocess.Popen(
            [str(binary), "-p", "hello"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0,
        )
        proc.wait(timeout=10)
    except:
        pass
    return 0

def benchmark_print_mode(name, binary, prompt="Explain what a binary search algorithm does in exactly 3 sentences", runs=3):
    """Benchmark print mode (non-interactive) response time."""
    times = []
    for i in range(runs):
        env = os.environ.copy()
        env["TERM"] = "xterm-256color"
        start = time.perf_counter()
        try:
            proc = subprocess.Popen(
                [str(binary), "-p", prompt],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=env,
                creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0,
            )
            stdout, stderr = proc.communicate(timeout=60)
            elapsed = time.perf_counter() - start
            times.append((elapsed, len(stdout)))
        except subprocess.TimeoutExpired:
            proc.kill()
            times.append((60.0, 0))
        except Exception as e:
            times.append((60.0, 0))
    return times

def run_benchmarks():
    """Run all benchmarks."""
    print("=" * 60)
    print("CAIRN-CODE BENCHMARK: Go vs Rust")
    print("=" * 60)
    print()

    # Binary sizes
    print("BINARY SIZE")
    print("-" * 40)
    go_size = GO_BIN.stat().st_size
    rust_size = RUST_BIN.stat().st_size
    print(f"  Go:   {go_size:>10,} bytes ({go_size / 1024 / 1024:.2f} MB)")
    print(f"  Rust: {rust_size:>10,} bytes ({rust_size / 1024 / 1024:.2f} MB)")
    print(f"  Ratio: {go_size / rust_size:.1f}x")
    print()

    # Startup time (print mode - no TUI)
    print("STARTUP TIME (print mode, -p flag)")
    print("-" * 40)
    print("  Measuring: time to complete 'say hello' prompt")
    go_times = measure_startup("Go", GO_BIN, runs=3)
    rust_times = measure_startup("Rust", RUST_BIN, runs=3)

    go_avg = statistics.mean(go_times)
    rust_avg = statistics.mean(rust_times)
    go_median = statistics.median(go_times)
    rust_median = statistics.median(rust_times)

    print(f"  Go:   avg={go_avg:.3f}s  median={go_median:.3f}s  min={min(go_times):.3f}s  max={max(go_times):.3f}s")
    print(f"  Rust: avg={rust_avg:.3f}s  median={rust_median:.3f}s  min={min(rust_times):.3f}s  max={max(rust_times):.3f}s")
    if go_avg > 0 and rust_avg > 0:
        speedup = go_avg / rust_avg
        print(f"  Rust is {speedup:.2f}x {'faster' if speedup > 1 else 'slower'}")
    print()

    # Print mode response time (requires API key)
    api_key = get_api_key()
    if api_key:
        print("API RESPONSE TIME (print mode, -p flag)")
        print("-" * 40)
        prompt = "Say hello in exactly 5 words"
        print(f"  Prompt: '{prompt}'")
        print(f"  Provider: OpenRouter")
        print()

        go_results = benchmark_print_mode("Go", GO_BIN, prompt, runs=2)
        rust_results = benchmark_print_mode("Rust", RUST_BIN, prompt, runs=2)

        for name, results in [("Go", go_results), ("Rust", rust_results)]:
            times = [r[0] for r in results if r[0] < 60]
            outputs = [r[1] for r in results if r[0] < 60]
            if times:
                avg_time = statistics.mean(times)
                avg_out = statistics.mean(outputs)
                print(f"  {name}:   avg={avg_time:.3f}s  output={avg_out:.0f} bytes")
                for i, (t, o) in enumerate(results):
                    if t < 60:
                        print(f"            run {i+1}: {t:.3f}s  {o} bytes")
            else:
                print(f"  {name}:   timeout or error")
        print()

        # Throughput test
        print("THROUGHPUT TEST (longer response)")
        print("-" * 40)
        prompt2 = "Write a 200-word essay about the history of computing"
        print(f"  Prompt: '{prompt2[:50]}...'")
        print()

        go_results2 = benchmark_print_mode("Go", GO_BIN, prompt2, runs=2)
        rust_results2 = benchmark_print_mode("Rust", RUST_BIN, prompt2, runs=2)

        for name, results in [("Go", go_results2), ("Rust", rust_results2)]:
            times = [r[0] for r in results if r[0] < 60]
            outputs = [r[1] for r in results if r[0] < 60]
            if times:
                avg_time = statistics.mean(times)
                avg_out = statistics.mean(outputs)
                throughput = avg_out / avg_time if avg_time > 0 else 0
                print(f"  {name}:   avg={avg_time:.3f}s  output={avg_out:.0f} bytes  throughput={throughput:.0f} bytes/s")
                for i, (t, o) in enumerate(results):
                    if t < 60:
                        tp = o / t if t > 0 else 0
                        print(f"            run {i+1}: {t:.3f}s  {o} bytes  {tp:.0f} bytes/s")
            else:
                print(f"  {name}:   timeout or error")
        print()
    else:
        print("API RESPONSE TIME (SKIPPED - no API key found)")
        print("  Set OPENROUTER_API_KEY env var or add to config.json")
        print()

    # Summary
    print("SUMMARY")
    print("-" * 40)
    print(f"  Binary size: Rust is {go_size / rust_size:.1f}x smaller")
    print(f"  Startup:     Rust is {go_avg / rust_avg:.2f}x {'faster' if go_avg > rust_avg else 'slower'}")
    print()
    print("Notes:")
    print("  - Startup time includes process creation + binary load")
    print("  - API response time depends on network and provider")
    print("  - Input latency requires interactive TUI measurement (manual)")
    print("  - Memory usage requires psutil (pip install psutil)")
    print("=" * 60)

if __name__ == "__main__":
    check_binaries()
    run_benchmarks()
