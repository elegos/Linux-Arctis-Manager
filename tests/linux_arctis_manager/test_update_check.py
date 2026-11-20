
from linux_arctis_manager.update_check import (
    check_for_update,
    fetch_releases,
    parse_releases,
)


def rel(tag: str, prerelease: bool = False, draft: bool = False) -> dict:
    return {'tag_name': tag, 'prerelease': prerelease, 'draft': draft}


RELEASES = [
    rel('v3.0.0-beta3', prerelease=True),
    rel('v3.0.0-beta4', prerelease=True),
    rel('v2.9.0'),
    rel('v2.8.0'),
    rel('v9.9.9', draft=True),
]


def test_parse_picks_latest_stable_and_prerelease_and_skips_drafts():
    stable, pre = parse_releases(RELEASES)
    assert stable == '2.9.0'
    assert pre == '3.0.0-beta4'


def test_parse_handles_missing_kinds_and_garbage_tags():
    assert parse_releases([]) == (None, None)
    assert parse_releases([rel('not-a-version'), rel('v1.0.0')]) == ('1.0.0', None)


def test_stable_user_ignores_prereleases():
    info = check_for_update('2.8.0', RELEASES)
    assert info.update_available
    assert info.target == '2.9.0'


def test_stable_user_up_to_date_even_if_prerelease_is_newer():
    info = check_for_update('2.9.0', RELEASES)
    assert not info.update_available
    assert info.target is None


def test_unstable_user_sees_newer_prerelease():
    info = check_for_update('3.0.0-beta3', RELEASES)
    assert info.update_available
    assert info.target == '3.0.0-beta4'


def test_unstable_user_sees_stable_release_that_supersedes_the_prerelease():
    info = check_for_update('3.0.0-beta4', [*RELEASES, rel('v3.0.0')])
    assert info.update_available
    assert info.target == '3.0.0'


def test_unstable_user_up_to_date():
    info = check_for_update('3.0.0-beta4', RELEASES)
    assert not info.update_available


def test_info_always_reports_both_channels():
    info = check_for_update('2.9.0', RELEASES)
    assert (info.stable, info.prerelease) == ('2.9.0', '3.0.0-beta4')


def test_unparseable_current_version_never_nags():
    assert not check_for_update('dev', RELEASES).update_available


def test_fetch_releases_returns_none_on_network_error(monkeypatch):
    import requests

    def boom(*a, **kw):
        raise requests.ConnectionError

    monkeypatch.setattr('linux_arctis_manager.update_check.requests.get', boom)
    assert fetch_releases() is None


def test_fetch_releases_returns_json(monkeypatch):
    class R:
        def raise_for_status(self): pass
        def json(self): return RELEASES

    monkeypatch.setattr('linux_arctis_manager.update_check.requests.get', lambda *a, **kw: R())
    assert fetch_releases() == RELEASES
