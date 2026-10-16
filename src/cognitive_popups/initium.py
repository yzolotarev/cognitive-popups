"""One persisted star per explicit study path. No network, scores or inferred knowledge."""
import hashlib
import math
import random

TINTS = ('#f3e0bd', '#cecbf6', '#b5d4f4', '#9fe1cb')
BIRTH_SECONDS = 3.55


def identity_for(key):
    seed = int(hashlib.sha256(key.encode()).hexdigest()[:12], 16)
    r = random.Random(seed)
    return dict(seed=seed, tint=TINTS[r.randrange(4)], rays=r.choice((4, 6)), rotation=.2 + r.random()*.5)


def progress(journey):
    done, target = journey['completed'], journey['target_steps']
    return f'{done}/{target}' if target else f'{done}/∞'


def at_end(journey):
    return bool(journey and journey['target_steps'] and journey['completed'] >= journey['target_steps'])


def smooth(t):
    t = max(0, min(1, t))
    return t*t*(3-2*t)


class Birth:
    """Reference-inspired damped attraction, inertial lift and organic orbit."""
    def __init__(self, identity, origin, corner):
        self.identity, self.origin, self.corner = identity, origin, corner
        r = random.Random(identity['seed'])
        x, y = origin
        self.particles = [dict(x=x+r.uniform(-145,145), y=y+90+r.random()*50,
            vx=r.uniform(-.5,.5), vy=-r.uniform(.7,2), angle=r.random()*math.tau,
            radius=r.uniform(8,90), delay=r.uniform(.08,.9), phase=r.random()*math.tau,
            size=r.uniform(.5,1.2)) for _ in range(210)]

    def pose(self, t):
        move = smooth((t-2.45)/1.05)
        x = self.origin[0]*(1-move)+self.corner[0]*move
        y = self.origin[1]*(1-move)+self.corner[1]*move-math.sin(move*math.pi)*45
        scale = (.25+1.1*smooth(t/.9))*(1-move)+.62*move
        return x, y, scale

    def advance(self, t, dt):
        dt = min(2, max(.1, dt*60))
        x,y,scale = self.pose(t)
        for p in self.particles:
            if t < p['delay']:
                continue
            k=smooth((t-p['delay']-.15)/.85)
            p['angle'] += .015*dt
            tx=x+math.cos(p['angle'])*p['radius']*scale*.75
            ty=y+math.sin(p['angle'])*p['radius']*scale*.5
            p['vx'] += ((tx-p['x'])*.022*k+math.cos(t+p['phase'])*.09*(1-k))*dt
            p['vy'] += ((ty-p['y'])*.022*k-.04*(1-k))*dt
            p['vx'] *= .93**dt; p['vy'] *= .93**dt
            p['x'] += p['vx']*dt; p['y'] += p['vy']*dt
