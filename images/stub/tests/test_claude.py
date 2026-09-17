"""Tests for the stub Claude Code CLI (`images/stub/claude`).

Run from the repository root with:

    python3 -m unittest discover -s images/stub/tests

Every test drives the script as a subprocess with its own temporary fixture,
and every wait has a timeout so a hung stub fails loudly instead of hanging
the suite.
"""

import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

STUB = Path(__file__).resolve().parents[1] / "claude"
FIXTURES = Path(__file__).resolve().parents[1] / "fixtures"
TIMEOUT = 10


def assistant(text):
    return {
        "type": "assistant",
        "message": {
            "id": "msg_fixture",
            "type": "message",
            "role": "assistant",
            "model": "fixture",
            "content": [{"type": "text", "text": text}],
        },
    }


def result(num_turns=1):
    return {
        "type": "result",
        "subtype": "success",
        "is_error": False,
        "num_turns": num_turns,
        "duration_ms": 12,
        "total_cost_usd": 0.002,
        "usage": {"input_tokens": 1, "output_tokens": 2},
        "permission_denials": [],
    }


def user_line(text):
    return json.dumps(
        {
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": text}]},
        }
    )


class StubTestCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def write_fixture(self, objects, name="fixture.jsonl"):
        path = self.tmp / name
        path.write_text("".join(json.dumps(o) + "\n" for o in objects), encoding="utf-8")
        return path

    def env(self, fixture=None, **extra):
        env = dict(os.environ)
        env.pop("CLAUDE_CONFIG_DIR", None)
        env.pop("MARS_STUB_LINE_DELAY_MS", None)
        env.pop("MARS_STUB_EXIT_AFTER_TURNS", None)
        env.pop("MARS_STUB_EXIT_CODE", None)
        env.pop("MARS_STUB_FIXTURE", None)
        if fixture is not None:
            env["MARS_STUB_FIXTURE"] = str(fixture)
        env.update({k: str(v) for k, v in extra.items()})
        return env

    def run_stub(self, args, fixture=None, stdin="", env_extra=None):
        completed = subprocess.run(
            [sys.executable, str(STUB)] + args,
            input=stdin,
            capture_output=True,
            text=True,
            timeout=TIMEOUT,
            env=self.env(fixture, **(env_extra or {})),
        )
        return completed

    def lines(self, completed):
        return [
            json.loads(line)
            for line in completed.stdout.splitlines()
            if line.strip()
        ]


class ArgumentTests(StubTestCase):
    def test_version_and_help_exit_zero(self):
        version = self.run_stub(["--version"])
        self.assertEqual(version.returncode, 0)
        self.assertEqual(version.stdout.strip(), "0.0.0-stub (Claude Code stub)")

        usage = self.run_stub(["--help"])
        self.assertEqual(usage.returncode, 0)
        self.assertIn("Claude Code stub", usage.stdout)

    def test_orchestrator_argv_is_accepted(self):
        fixture = self.write_fixture([assistant("hello"), result()])
        completed = self.run_stub(
            [
                "--print",
                "--output-format", "stream-json",
                "--input-format", "stream-json",
                "--verbose",
                "--forward-subagent-text",
                "--system-prompt-snapshot", "off",
                "--include-partial-messages",
                "--permission-mode", "bypassPermissions",
                "--permission-prompts", "none",
                "--strict-mcp-config",
                "--bare",
                "--model=stub-model",
                "--append-system-prompt", "be brief",
            ],
            fixture=fixture,
            stdin="",
        )
        self.assertEqual(completed.returncode, 0)
        self.assertEqual(len(self.lines(completed)), 1)
        self.assertNotIn("unknown flag", completed.stderr)

    def test_unknown_flag_warns_once_and_is_ignored(self):
        fixture = self.write_fixture([assistant("hi"), result()])
        completed = self.run_stub(
            ["-p", "--nonsense", "--nonsense", "prompt"], fixture=fixture
        )
        self.assertEqual(completed.returncode, 0)
        self.assertEqual(completed.stderr.count("--nonsense"), 1)


class InitTests(StubTestCase):
    def test_init_is_first_and_carries_a_fresh_session_id(self):
        fixture = self.write_fixture([assistant("hi"), result()])
        completed = self.run_stub(["-p", "hello"], fixture=fixture)
        lines = self.lines(completed)
        init = lines[0]
        self.assertEqual(init["type"], "system")
        self.assertEqual(init["subtype"], "init")
        self.assertEqual(init["cwd"], "/session/work")
        self.assertEqual(
            init["tools"], ["Bash", "Read", "Edit", "Write", "Glob", "Grep", "Task"]
        )
        self.assertEqual(init["mcp_servers"], [])
        self.assertEqual(init["model"], "stub")
        self.assertEqual(init["permissionMode"], "bypassPermissions")
        self.assertEqual(len(init["session_id"]), 36)
        for line in lines:
            self.assertEqual(line["session_id"], init["session_id"])

    def test_resume_reuses_the_session_id(self):
        fixture = self.write_fixture([assistant("hi"), result()])
        completed = self.run_stub(
            ["-p", "hello", "--resume", "sess-0001-stub"], fixture=fixture
        )
        lines = self.lines(completed)
        self.assertEqual(lines[0]["session_id"], "sess-0001-stub")
        self.assertTrue(all(line["session_id"] == "sess-0001-stub" for line in lines))

    def test_mcp_servers_come_from_the_config_file(self):
        fixture = self.write_fixture([result()])
        config = self.tmp / "mcp.json"
        config.write_text(
            json.dumps(
                {
                    "mcpServers": {
                        "mars-orchestrator": {
                            "type": "http",
                            "url": "http://orchestrator:7001/mcp",
                            "headers": {"Authorization": "Bearer not-a-real-token"},
                        }
                    }
                }
            ),
            encoding="utf-8",
        )
        completed = self.run_stub(
            ["-p", "hello", "--mcp-config", str(config)], fixture=fixture
        )
        init = self.lines(completed)[0]
        self.assertEqual(
            init["mcp_servers"], [{"name": "mars-orchestrator", "status": "connected"}]
        )
        self.assertNotIn("not-a-real-token", completed.stdout)
        self.assertNotIn("not-a-real-token", completed.stderr)

    def test_unreadable_mcp_config_warns_and_yields_no_servers(self):
        fixture = self.write_fixture([result()])
        missing = self.tmp / "absent.json"
        completed = self.run_stub(
            ["-p", "hello", "--mcp-config", str(missing)], fixture=fixture
        )
        self.assertEqual(completed.returncode, 0)
        self.assertEqual(self.lines(completed)[0]["mcp_servers"], [])
        self.assertIn("mcp config unreadable", completed.stderr)


class OneShotTests(StubTestCase):
    def test_all_turns_are_emitted_back_to_back(self):
        fixture = self.write_fixture(
            [assistant("one"), result(1), assistant("two"), result(2)]
        )
        completed = self.run_stub(["-p", "do it"], fixture=fixture)
        self.assertEqual(completed.returncode, 0)
        kinds = [line["type"] for line in self.lines(completed)]
        self.assertEqual(
            kinds, ["system", "assistant", "result", "assistant", "result"]
        )

    def test_prompt_is_read_from_stdin_without_a_positional(self):
        fixture = self.write_fixture([])
        completed = self.run_stub(["-p"], fixture=fixture, stdin="prompt on stdin\n")
        lines = self.lines(completed)
        self.assertEqual(completed.returncode, 0)
        self.assertEqual(
            lines[1]["message"]["content"][0]["text"],
            "Stub reply to: prompt on stdin",
        )
        self.assertEqual(lines[-1]["type"], "result")

    def test_trailing_turn_without_a_result_gets_one(self):
        fixture = self.write_fixture([assistant("one"), result(1), assistant("two")])
        completed = self.run_stub(["-p", "do it"], fixture=fixture)
        lines = self.lines(completed)
        self.assertEqual([line["type"] for line in lines][-2:], ["assistant", "result"])
        self.assertEqual(lines[-1]["num_turns"], 1)
        self.assertEqual(lines[-1]["subtype"], "success")

    def test_malformed_fixture_line_is_skipped_with_a_warning(self):
        path = self.tmp / "broken.jsonl"
        path.write_text(
            json.dumps(assistant("one")) + "\n{not json\n" + json.dumps(result()) + "\n",
            encoding="utf-8",
        )
        completed = self.run_stub(["-p", "do it"], fixture=path)
        self.assertEqual(completed.returncode, 0)
        self.assertIn("malformed fixture line 2", completed.stderr)
        self.assertEqual(
            [line["type"] for line in self.lines(completed)],
            ["system", "assistant", "result"],
        )

    def test_fixture_init_lines_are_replaced_by_the_stub_s_own(self):
        fixture = self.write_fixture(
            [
                {"type": "system", "subtype": "init", "session_id": "from-fixture"},
                assistant("one"),
                result(),
            ]
        )
        completed = self.run_stub(["-p", "do it"], fixture=fixture)
        lines = self.lines(completed)
        self.assertEqual(len(lines), 3)
        self.assertNotEqual(lines[0]["session_id"], "from-fixture")


class InteractiveTests(StubTestCase):
    def test_one_turn_per_stdin_line_then_synthesised_turns(self):
        fixture = self.write_fixture([assistant("recorded"), result()])
        completed = self.run_stub(
            ["--print", "--input-format", "stream-json"],
            fixture=fixture,
            stdin=user_line("first") + "\n" + user_line("second") + "\n",
        )
        self.assertEqual(completed.returncode, 0)
        lines = self.lines(completed)
        self.assertEqual(
            [line["type"] for line in lines],
            ["system", "assistant", "result", "assistant", "result"],
        )
        self.assertEqual(lines[1]["message"]["content"][0]["text"], "recorded")
        self.assertEqual(
            lines[3]["message"]["content"][0]["text"], "Stub reply to: second"
        )
        synthesised = lines[4]
        self.assertEqual(synthesised["subtype"], "success")
        self.assertFalse(synthesised["is_error"])
        self.assertEqual(synthesised["num_turns"], 1)
        self.assertEqual(synthesised["total_cost_usd"], 0.001)
        self.assertEqual(
            synthesised["usage"], {"input_tokens": 10, "output_tokens": 5}
        )
        self.assertEqual(synthesised["permission_denials"], [])
        self.assertIn("duration_ms", synthesised)

    def test_interactive_wins_over_a_positional_prompt(self):
        fixture = self.write_fixture([assistant("recorded"), result()])
        completed = self.run_stub(
            ["--print", "--input-format", "stream-json", "a prompt"],
            fixture=fixture,
            stdin="",
        )
        self.assertEqual(completed.returncode, 0)
        self.assertEqual(len(self.lines(completed)), 1)

    def test_blank_and_malformed_stdin_lines_consume_a_turn(self):
        fixture = self.write_fixture([])
        completed = self.run_stub(
            ["--print", "--input-format", "stream-json"],
            fixture=fixture,
            stdin="\nnot json at all\n",
        )
        self.assertEqual(completed.returncode, 0)
        lines = self.lines(completed)
        self.assertEqual(
            [line["type"] for line in lines],
            ["system", "assistant", "result", "assistant", "result"],
        )
        self.assertEqual(lines[1]["message"]["content"][0]["text"], "Stub reply to: ")
        self.assertEqual(
            lines[3]["message"]["content"][0]["text"],
            "Stub reply to: not json at all",
        )

    def test_empty_fixture_one_shot_emits_init_and_one_result(self):
        fixture = self.write_fixture([])
        completed = self.run_stub(["-p", "hello"], fixture=fixture)
        lines = self.lines(completed)
        self.assertEqual(completed.returncode, 0)
        self.assertEqual([line["type"] for line in lines], ["system", "assistant", "result"])


class PartialMessageTests(StubTestCase):
    def fixture_with_stream_event(self):
        return self.write_fixture(
            [
                {
                    "type": "stream_event",
                    "event": {
                        "type": "content_block_delta",
                        "delta": {"type": "text_delta", "text": "par"},
                    },
                },
                assistant("partial"),
                result(),
            ]
        )

    def test_stream_events_are_dropped_by_default(self):
        completed = self.run_stub(["-p", "hi"], fixture=self.fixture_with_stream_event())
        self.assertEqual(
            [line["type"] for line in self.lines(completed)],
            ["system", "assistant", "result"],
        )

    def test_stream_events_are_emitted_with_include_partial_messages(self):
        completed = self.run_stub(
            ["-p", "hi", "--include-partial-messages"],
            fixture=self.fixture_with_stream_event(),
        )
        self.assertEqual(
            [line["type"] for line in self.lines(completed)],
            ["system", "stream_event", "assistant", "result"],
        )


class ExitControlTests(StubTestCase):
    def test_exit_after_turns_uses_the_configured_exit_code(self):
        fixture = self.write_fixture(
            [assistant("one"), result(1), assistant("two"), result(2)]
        )
        completed = self.run_stub(
            ["-p", "do it"],
            fixture=fixture,
            env_extra={"MARS_STUB_EXIT_AFTER_TURNS": 1, "MARS_STUB_EXIT_CODE": 7},
        )
        self.assertEqual(completed.returncode, 7)
        self.assertEqual(
            [line["type"] for line in self.lines(completed)],
            ["system", "assistant", "result"],
        )

    def test_exit_after_turns_defaults_to_exit_code_one(self):
        fixture = self.write_fixture([assistant("one"), result(1)])
        completed = self.run_stub(
            ["-p", "do it"],
            fixture=fixture,
            env_extra={"MARS_STUB_EXIT_AFTER_TURNS": 1},
        )
        self.assertEqual(completed.returncode, 1)

    def test_missing_fixture_exits_one_before_any_output(self):
        missing = self.tmp / "absent.jsonl"
        completed = self.run_stub(["-p", "hi"], fixture=missing)
        self.assertEqual(completed.returncode, 1)
        self.assertEqual(completed.stdout, "")
        self.assertIn("stub: fixture not found: " + str(missing), completed.stderr)


class TranscriptTests(StubTestCase):
    def test_every_line_is_copied_under_claude_config_dir(self):
        fixture = self.write_fixture([assistant("one"), result()])
        config_dir = self.tmp / "claude"
        completed = self.run_stub(
            ["-p", "hi", "--resume", "sess-0002-stub"],
            fixture=fixture,
            env_extra={"CLAUDE_CONFIG_DIR": str(config_dir)},
        )
        self.assertEqual(completed.returncode, 0)
        transcript = config_dir / "projects" / "-session-work" / "sess-0002-stub.jsonl"
        self.assertTrue(transcript.exists())
        self.assertEqual(
            transcript.read_text(encoding="utf-8").splitlines(),
            completed.stdout.splitlines(),
        )
        self.assertIn("no transcript to resume", completed.stderr)


class ShippedFixtureTests(StubTestCase):
    """The fixtures the image ships under /opt/mars-stub/fixtures/.

    They are loaded through MARS_STUB_FIXTURE exactly as the container does, so
    a change to either file that the stub cannot replay fails here rather than
    in an end-to-end run (`images/stub/fixtures/README.md`).
    """

    def test_default_fixture_replays_three_turns_with_partial_messages(self):
        completed = self.run_stub(
            ["-p", "hello", "--include-partial-messages"],
            fixture=FIXTURES / "default.jsonl",
        )
        self.assertEqual(completed.returncode, 0)
        lines = self.lines(completed)
        self.assertEqual(len(lines), 26)
        self.assertEqual(lines[0]["type"], "system")
        self.assertEqual(lines[0]["subtype"], "init")
        results = [line for line in lines if line["type"] == "result"]
        self.assertEqual(len(results), 3)
        self.assertEqual(
            [line["total_cost_usd"] for line in results], [0.0123, 0.0456, 0.0089]
        )
        self.assertEqual(lines[-1]["type"], "result")
        self.assertEqual(
            len([line for line in lines if line["type"] == "stream_event"]), 2
        )

    def test_default_fixture_drops_stream_events_without_the_flag(self):
        completed = self.run_stub(["-p", "hello"], fixture=FIXTURES / "default.jsonl")
        self.assertEqual(completed.returncode, 0)
        lines = self.lines(completed)
        self.assertEqual([line["type"] for line in lines if line["type"] == "stream_event"], [])
        self.assertEqual(len(lines), 24)
        self.assertEqual(len([line for line in lines if line["type"] == "result"]), 3)

    def test_agent_tool_fixture_replays_one_turn(self):
        completed = self.run_stub(
            ["-p", "hello"], fixture=FIXTURES / "agent-tool.jsonl"
        )
        self.assertEqual(completed.returncode, 0)
        lines = self.lines(completed)
        self.assertEqual(lines[0]["subtype"], "init")
        self.assertEqual(len([line for line in lines if line["type"] == "result"]), 1)
        self.assertEqual(lines[-1]["type"], "result")
        tool_names = [
            block.get("name")
            for line in lines
            if line["type"] == "assistant"
            for block in line["message"]["content"]
            if block.get("type") == "tool_use"
        ]
        self.assertIn("Agent", tool_names)


class SignalTests(StubTestCase):
    def spawn(self, args, fixture, **env_extra):
        proc = subprocess.Popen(
            [sys.executable, str(STUB)] + args,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=self.env(fixture, **env_extra),
        )
        self.addCleanup(proc.kill)
        self.addCleanup(self.close_pipes, proc)
        return proc

    @staticmethod
    def close_pipes(proc):
        for stream in (proc.stdin, proc.stdout, proc.stderr):
            try:
                stream.close()
            except OSError:
                pass

    def start_reader(self, proc):
        collected = []

        def reader():
            for line in proc.stdout:
                collected.append(line)

        thread = threading.Thread(target=reader, daemon=True)
        thread.start()
        return collected, thread

    def wait_for_lines(self, collected, count):
        deadline = time.monotonic() + TIMEOUT
        while time.monotonic() < deadline:
            if len(collected) >= count:
                return
            time.sleep(0.02)
        self.fail("stub produced only %d lines: %r" % (len(collected), collected))

    def test_sigint_mid_turn_writes_a_result_and_exits_zero(self):
        fixture = self.write_fixture(
            [assistant("a"), assistant("b"), assistant("c"), assistant("d"), result(7)]
        )
        proc = self.spawn(
            ["--print", "--input-format", "stream-json"],
            fixture,
            MARS_STUB_LINE_DELAY_MS=200,
        )
        collected, thread = self.start_reader(proc)
        self.wait_for_lines(collected, 1)
        proc.stdin.write(user_line("go") + "\n")
        proc.stdin.flush()
        self.wait_for_lines(collected, 2)
        proc.send_signal(signal.SIGINT)
        self.assertEqual(proc.wait(timeout=TIMEOUT), 0)
        thread.join(timeout=TIMEOUT)

        lines = [json.loads(line) for line in collected if line.strip()]
        self.assertLess(len(lines), 6, "the interrupted turn should not finish")
        closing = lines[-1]
        self.assertEqual(closing["type"], "result")
        self.assertEqual(closing["subtype"], "success")
        self.assertFalse(closing["is_error"])
        self.assertEqual(closing["num_turns"], 1)
        self.assertEqual(closing["session_id"], lines[0]["session_id"])
        self.assertEqual(closing["total_cost_usd"], 0.001)
        self.assertEqual(closing["usage"], {"input_tokens": 10, "output_tokens": 5})
        self.assertIn("duration_ms", closing)

    def test_sigint_while_waiting_for_stdin_exits_zero(self):
        fixture = self.write_fixture([assistant("a"), result()])
        proc = self.spawn(["--print", "--input-format", "stream-json"], fixture)
        self.assertIn("\"subtype\":\"init\"", proc.stdout.readline())
        proc.send_signal(signal.SIGINT)
        self.assertEqual(proc.wait(timeout=TIMEOUT), 0)
        self.assertEqual(proc.stdout.read().strip(), "")

    def test_sigterm_exits_143_without_a_result(self):
        fixture = self.write_fixture([assistant("a"), result()])
        proc = self.spawn(["--print", "--input-format", "stream-json"], fixture)
        self.assertIn("\"subtype\":\"init\"", proc.stdout.readline())
        proc.terminate()
        self.assertEqual(proc.wait(timeout=TIMEOUT), 143)
        self.assertEqual(proc.stdout.read().strip(), "")


if __name__ == "__main__":
    unittest.main()
