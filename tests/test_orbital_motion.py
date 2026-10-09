import pytest
from cognitive_popups.orbital import Rect
from cognitive_popups.orbital_motion import Tween, ease, rect_values, visual_rect


def test_cubic_fade_monotonic_and_exact_endpoints():
    values = [ease(i / 100) for i in range(-10, 111)]
    assert values == sorted(values)
    assert values[0] == 0 and values[-1] == 1
    assert ease(.5) == .875


def test_fakeclock_rect_interpolates_all_four_axes():
    start, target = Rect(342, 180, 115, 105), Rect(260, 12, 280, 230)
    tween = Tween(rect_values(start), rect_values(target), 10., .22)
    assert visual_rect(tween.value(10.)) == start
    middle = tween.value(10.11)
    assert middle == pytest.approx(tuple(a + (b-a)*.875 for a,b in zip(rect_values(start),rect_values(target))))
    assert not tween.done(10.219)
    assert tween.done(10.22)
    assert visual_rect(tween.value(10.22)) == target


def test_rapid_retarget_samples_current_visual_not_previous_target():
    tween = Tween((0., 0., 10., 10.), (100., 200., 80., 70.), 0., .22)
    current = tween.value(.05)
    tween.retarget((300., 0., 280., 230.), .05)
    assert tween.value(.05) == current
    assert tween.started == .05
    assert tween.value(.27) == (300., 0., 280., 230.)
    tween.retarget((10., 40., 100., 100.), .27)
    assert tween.value(.27) == (300., 0., 280., 230.)
    assert tween.done(.49)


def test_rounding_never_produces_empty_membrane():
    assert visual_rect((1.1, 2.7, .2, .3)) == Rect(1, 3, 1, 1)


def test_dim_and_rect_have_independent_durations():
    rect = Tween((0.,), (1.,), 0., .22)
    dim = Tween((1.,), (.62,), 0., .14)
    assert dim.done(.14) and not rect.done(.14)
    assert dim.value(.14) == (.62,)
