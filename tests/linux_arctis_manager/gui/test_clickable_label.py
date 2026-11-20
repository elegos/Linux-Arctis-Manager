import os
import sys

os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')

from PySide6.QtCore import QPoint, Qt
from PySide6.QtTest import QTest
from PySide6.QtWidgets import QApplication

_app = QApplication.instance() or QApplication(sys.argv)

from linux_arctis_manager.gui.clickable_label import QClickableLabel  # noqa: E402


def test_left_click_emits_clicked():
    label = QClickableLabel('v1')
    hits: list[int] = []
    label.clicked.connect(lambda: hits.append(1))
    QTest.mouseClick(label, Qt.MouseButton.LeftButton, pos=QPoint(1, 1))
    assert hits == [1]


def test_right_click_is_ignored_and_text_still_settable():
    label = QClickableLabel('v1')
    hits: list[int] = []
    label.clicked.connect(lambda: hits.append(1))
    QTest.mouseClick(label, Qt.MouseButton.RightButton, pos=QPoint(1, 1))
    label.setText('v2')
    assert hits == [] and label.text() == 'v2'
