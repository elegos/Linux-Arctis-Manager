from PySide6.QtCore import (Property, QEasingCurve, QPoint, QPropertyAnimation,
                            QRect, Qt)
from PySide6.QtGui import QPainter, QPaintEvent
from PySide6.QtWidgets import QCheckBox, QWidget

LEFT_MARGIN = 3

class QToggle(QCheckBox):
    def __init__(self, parent: QWidget|None = None, width: int = 60, is_checkbox: bool = False):
        '''
        Args:
            parent: The parent widget
            width: The width of the toggle
            is_checkbox: If the toggle is a checkbox, then it will use an accent color when enabled
        '''
        super().__init__(parent)

        self.setFixedSize(width, 28)
        self.setCursor(Qt.CursorShape.PointingHandCursor)
        self.is_checkbox = is_checkbox

        self._circle_position = LEFT_MARGIN
        self.animation = QPropertyAnimation(self, b'circle_position', self)
        self.animation.setEasingCurve(QEasingCurve.Type.OutExpo)
        self.animation.setDuration(500)

        self.checkStateChanged.connect(self.start_animation_transition)

    def start_animation_transition(self, value: bool):
        self.animation.stop()

        if value == Qt.CheckState.Checked:
            self.animation.setEndValue(self.width() - LEFT_MARGIN - 22)
        else:
            self.animation.setEndValue(LEFT_MARGIN)
        
        self.animation.start()

    @Property(float)
    def circle_position(self) -> float:
        return self._circle_position
    
    @circle_position.setter
    def circle_position(self, value: float):
        self._circle_position = value
        self.update()

    
    def hitButton(self, pos: QPoint) -> bool:
        return self.contentsRect().contains(pos)

    def paintEvent(self, event: QPaintEvent):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.setPen(Qt.PenStyle.NoPen)
        
        box = QRect(0, 0, self.width(), self.height())

        # Background
        painter.setBrush(self.palette().accent() if self.is_checkbox and self.isChecked() else self.palette().base())
        painter.drawRoundedRect(0, 0, self.width(), self.height(), self.height() / 2, self.height() / 2)

        # Status circle
        painter.setBrush(self.palette().buttonText())
        painter.drawEllipse(self._circle_position, 3, 22, 22)

        painter.end()
