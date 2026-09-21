"""`with client.run(...)` launches, hands over the CDP endpoint, and stops."""

from __future__ import annotations

import pytest
from fake_bwbrowser import FakeBwbrowser

from bwbrowser import Conflict, BwbrowserClient, BwbrowserError

RUN_BODY = {"profile_id": "p1", "remote_debugging_port": 9222, "headless": True}


def test_the_block_gets_the_cdp_endpoint(client: BwbrowserClient, fake: FakeBwbrowser) -> None:
    fake.enqueue_json(RUN_BODY)
    fake.enqueue_empty(204)

    with client.run("p1", url="https://example.com", headless=True) as session:
        assert session.remote_debugging_port == 9222
        assert session.headless is True
        assert session.cdp_url == "http://127.0.0.1:9222"
        assert session.response == RUN_BODY

    assert [(sent.method, sent.path) for sent in fake.requests] == [
        ("POST", "/v1/profiles/p1/run"),
        ("POST", "/v1/profiles/p1/kill"),
    ]
    assert fake.requests[0].json == {"url": "https://example.com", "headless": True}


def test_nothing_launches_until_the_block_is_entered(
    client: BwbrowserClient, fake: FakeBwbrowser
) -> None:
    session = client.run("p1")
    assert session.remote_debugging_port is None
    assert fake.requests == []


def test_the_browser_is_stopped_when_the_block_raises(
    client: BwbrowserClient, fake: FakeBwbrowser
) -> None:
    fake.enqueue_json(RUN_BODY)
    fake.enqueue_empty(204)

    with pytest.raises(ZeroDivisionError):
        with client.run("p1"):
            raise ZeroDivisionError("the body failed")

    assert [sent.path for sent in fake.requests] == [
        "/v1/profiles/p1/run",
        "/v1/profiles/p1/kill",
    ]


def test_a_failed_stop_never_hides_why_the_block_failed(
    client: BwbrowserClient, fake: FakeBwbrowser
) -> None:
    fake.enqueue_json(RUN_BODY)
    fake.enqueue_error(409, "PROFILE_LOCKED_ELSEWHERE")

    session = client.run("p1")
    with pytest.raises(ZeroDivisionError):
        with session:
            raise ZeroDivisionError("the body failed")

    assert isinstance(session.cleanup_error, Conflict)


def test_a_failed_stop_is_raised_when_the_block_was_fine(
    client: BwbrowserClient, fake: FakeBwbrowser
) -> None:
    fake.enqueue_json(RUN_BODY)
    fake.enqueue_error(503, "the fleet could not be reached")

    with pytest.raises(BwbrowserError):
        with client.run("p1"):
            pass


def test_a_failed_launch_stops_nothing(client: BwbrowserClient, fake: FakeBwbrowser) -> None:
    fake.enqueue_error(409, "PROFILE_RUNNING")

    with pytest.raises(Conflict):
        with client.run("p1"):
            pytest.fail("the block must not run when the launch failed")

    assert [sent.path for sent in fake.requests] == ["/v1/profiles/p1/run"]


def test_the_cdp_url_is_refused_before_the_block(client: BwbrowserClient) -> None:
    session = client.run("p1")
    with pytest.raises(BwbrowserError):
        _ = session.cdp_url
