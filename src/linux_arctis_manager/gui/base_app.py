from PySide6.QtCore import QObject, Slot


class QBaseDesktopApp(QObject):
    @Slot()
    def sig_stop(self):
        raise NotImplementedError("This method should be implemented by subclasses")

    def start(self):
        raise NotImplementedError("This method should be implemented by subclasses")
    