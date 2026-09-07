BATTERY_BAR_WIDTH = 10
CHATMIX_BAR_WIDTH = 11


def battery_bar(value: int | float, width: int = BATTERY_BAR_WIDTH) -> str:
    """Render a 0-100 percentage as a filled/empty block bar."""
    value = max(0, min(100, value))
    filled = round(value / 100 * width)
    return '█' * filled + '░' * (width - filled)


def chatmix_bar(value: int | float, width: int = CHATMIX_BAR_WIDTH) -> str:
    """Render a 0 (game) - 100 (chat) balance as a marker on a track."""
    value = max(0, min(100, value))
    pos = round(value / 100 * (width - 1))
    return ''.join('●' if i == pos else '·' for i in range(width))


def chatmix_balance(game: int | float, chat: int | float) -> float:
    """Reduce independent game/chat mix levels to one 0 (game) - 100 (chat)
    position, for chatmix_bar(). Devices don't expose a single balance value
    — only two independently-reported levels — but SteelSeries' ChatMix
    dial is a crossfader in practice, so game+chat is expected to add up to
    ~100; this holds even when it doesn't (falls back to 50/50 on 0+0)."""
    total = game + chat
    return (chat / total * 100) if total > 0 else 50
