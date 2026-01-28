from PySide6.QtCore import Signal
from PySide6.QtWidgets import QWidget


class QMainAppProtoWidget(QWidget):
    visibilityChanged = Signal(bool)

    def hideEvent(self, event):
        super().hideEvent(event)

        self.visibilityChanged.emit(False)

    def showEvent(self, event):
        super().showEvent(event)

        self.visibilityChanged.emit(True)
