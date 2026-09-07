"""Tests for the battery/chat-mix ASCII bar helpers.

Keep in sync with the JS ports in packaging/plasma6/.../code/format.js and
packaging/gnome-shell/.../lib/format.js.
"""

from linux_arctis_manager.gui.ascii_bar import battery_bar, chatmix_balance, chatmix_bar


def test_battery_bar_empty():
    assert battery_bar(0) == '░' * 10


def test_battery_bar_full():
    assert battery_bar(100) == '█' * 10


def test_battery_bar_partial():
    assert battery_bar(82) == '█' * 8 + '░' * 2


def test_battery_bar_clamps_out_of_range_values():
    assert battery_bar(-10) == battery_bar(0)
    assert battery_bar(150) == battery_bar(100)


def test_chatmix_bar_game_side():
    bar = chatmix_bar(0)
    assert bar[0] == '●'
    assert bar.count('●') == 1


def test_chatmix_bar_chat_side():
    bar = chatmix_bar(100)
    assert bar[-1] == '●'
    assert bar.count('●') == 1


def test_chatmix_bar_balanced_is_centered():
    bar = chatmix_bar(50)
    assert bar[len(bar) // 2] == '●'
    assert bar.count('●') == 1


def test_chatmix_balance_all_game():
    assert chatmix_balance(game=100, chat=0) == 0


def test_chatmix_balance_all_chat():
    assert chatmix_balance(game=0, chat=100) == 100


def test_chatmix_balance_even_split():
    assert chatmix_balance(game=50, chat=50) == 50


def test_chatmix_balance_defaults_to_center_when_both_zero():
    assert chatmix_balance(game=0, chat=0) == 50
