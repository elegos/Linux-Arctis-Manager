from linux_arctis_manager.status_parser_fn import int_int_mapping, int_str_mapping, on_off, percentage


def test_percentage():
    fn = percentage
    assert getattr(fn, '_status_type') == 'percentage'

    assert fn(0, 100, 0) == 0
    assert fn(-56, 0, -56) == 0

    assert fn(0, 100, 75) == 75
    assert fn(-200, 0, -50) == 75

    assert fn(0, 100, 100) == 100
    assert fn(-123, 123, 123) == 100

def test_on_off():
    fn = on_off
    assert getattr(fn, '_status_type') == 'on_off'

    assert fn(0x01, 0x01, 0) is True
    assert fn(0, 1, 0) is False
    assert fn(1, 1, 3) is True
    assert fn(3, 2, 3) is False

def test_int_str_mapping():
    fn = int_str_mapping
    mapping = {0x00: "off", 0x01: "-12db", 0x02: "on"}

    assert getattr(fn, '_status_type') == 'int_str_mapping'

    assert fn(mapping, 0x00) == "off"
    assert fn(mapping, 0x01) == "-12db"
    assert fn(mapping, 0x02) == "on"
    assert fn(mapping, 0x03) is None

def test_int_int_mapping():
    fn = int_int_mapping
    mapping = {0: 10, 1: 20, 2: 30}

    assert getattr(fn, '_status_type') == 'int_int_mapping'

    assert fn(mapping, 0) == 10
    assert fn(mapping, 1) == 20
    assert fn(mapping, 2) == 30
    assert fn(mapping, 3) is None
