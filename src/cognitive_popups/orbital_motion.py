"""Clock-independent, interruptible Orbital motion in logical pixels."""
from dataclasses import dataclass
from .orbital import Rect


def ease(progress):
    t = min(1., max(0., progress))
    return 1. - (1. - t) ** 3


@dataclass
class Tween:
    start: tuple
    target: tuple
    started: float
    duration: float

    def value(self, now):
        p = ease((now - self.started) / self.duration)
        return tuple(a + (b - a) * p for a, b in zip(self.start, self.target))

    def done(self, now):
        return now >= self.started + self.duration

    def retarget(self, target, now, duration=None):
        self.start = self.value(now)
        self.target = tuple(target)
        self.started = now
        if duration is not None:
            self.duration = duration


def rect_values(rect):
    return (rect.x, rect.y, rect.width, rect.height)


def visual_rect(values):
    x, y, w, h = values
    return Rect(round(x), round(y), max(1, round(w)), max(1, round(h)))
