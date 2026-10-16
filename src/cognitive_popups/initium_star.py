"""Native GTK/Cairo Initium surface; empty input shape during birth and reading.

The desktop owns one instance. No browser, nested main loop or model work.
"""
import ctypes as C
import math
import threading
import time

from . import motion, theme
from .initium import Birth, BIRTH_SECONDS, birth_plays, progress, smooth
from .sand_canvas import connect_draw, rgb


class InitiumStar:
    WIDTH, HEIGHT = 132, 116

    def __init__(self, journey, *, birth=True, position=None):
        import gi
        gi.require_version('Gtk','3.0')
        from gi.repository import Gtk,Gdk,GLib,Pango
        self.Gtk,self.Gdk,self.GLib = Gtk,Gdk,GLib
        self.journey=journey
        self.identity=journey['identity']
        self.closed=False
        self.reduced=motion.reduced(Gtk)
        self.birthing=birth_plays(birth,self.reduced)
        self.started=self.last=time.monotonic()
        self.pulse_at=-100.; self.hover=False; self.mouse=None
        self.new_at={}; self.thoughts=[]; self.frames=0; self.first_frame=None
        self.audio=None; self.audio_source=None; self.audio_generation=0
        self.tick_id=None; self.poll_id=None
        self.window=Gtk.Window(title='Initium')
        # Reuse the installed owned sand surface rules (no compositor animations).
        self.window.set_wmclass('cognitive-sand','cognitive-sand')
        self.window.set_decorated(False); self.window.set_resizable(False)
        self.window.set_accept_focus(False); self.window.set_focus_on_map(False)
        self.window.set_keep_above(True)
        self.window.set_skip_taskbar_hint(True); self.window.set_skip_pager_hint(True)
        self.window.set_type_hint(Gdk.WindowTypeHint.NOTIFICATION)
        self.window.set_app_paintable(True)
        visual=self.window.get_screen().get_rgba_visual()
        if visual: self.window.set_visual(visual)
        css=Gtk.CssProvider()
        css.load_from_data(b'window { background-color: transparent; box-shadow: none; }')
        self.window.get_style_context().add_provider(css,Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION+200)
        self.area=Gtk.DrawingArea(); self.window.add(self.area)
        display=Gdk.Display.get_default(); seat=display.get_default_seat()
        self.pointer=seat.get_pointer()
        _,px,py=self.pointer.get_position()
        if position is not None:
            px,py=position
        monitor=display.get_monitor_at_point(px,py) or display.get_monitor(0)
        self.monitor=monitor
        a=monitor.get_workarea(); self.work=(a.x,a.y,a.width,a.height)
        self.width,self.height=(a.width,a.height) if self.birthing else (self.WIDTH,self.HEIGHT)
        self.home=(a.x+max(0,a.width-self.WIDTH-20),a.y+min(162,max(0,a.height-self.HEIGHT)))
        # Origin near the pointer; never place the whole compact object over text.
        self.model=Birth(self.identity,(max(50,min(a.width-50,px-a.x+24)),max(50,min(a.height-140,py-a.y-24))),
                         (self.home[0]-a.x+62,self.home[1]-a.y+48))
        self.area.set_size_request(self.width,self.height)
        self.window.move(a.x,a.y) if self.birthing else self.window.move(*self.home)
        self.layout=self.area.create_pango_layout(progress(journey))
        self.layout.set_font_description(Pango.FontDescription.from_string(theme.pango(theme.SMALL)))
        self.draw_handle=connect_draw(self.area,self.paint)
        self.window.connect('map',self._mapped)
        self.window.connect('delete-event',lambda *_: (self.close(),True)[1])
        self.window.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
        self.window.connect('button-press-event',lambda *_: (self.pulse(sound=True),True)[1])
        self.window.show_all()
        self.poll_id=GLib.timeout_add(180,self.poll)
        if self.birthing: self.animate()
        # Sound initialisation runs after the first visual frame, off the GTK thread.
        if birth: self.audio_source=GLib.timeout_add(40,self._start_audio)

    def shape(self, interactive=False):
        # The existing orbital region API contains an intentional centre hole.
        # Use its native bindings but a circle here, rather than a window rectangle.
        from .orbital_assets import _shape_api
        c,cairo,gdk,Rect=_shape_api()
        region=cairo.cairo_region_create()
        try:
            if interactive and not self.birthing:
                for y in range(34,63):
                    dx=int(math.sqrt(max(0,14**2-(y-48)**2)))
                    cairo.cairo_region_union_rectangle(region,c.byref(Rect(62-dx,y,2*dx+1,1)))
            gdk.gdk_window_input_shape_combine_region(hash(self.window.get_window()),region,0,0)
        finally: cairo.cairo_region_destroy(region)

    def _mapped(self,*_):
        self.shape()
        self.window.move(*((self.work[0],self.work[1]) if self.birthing else self.home))

    def animate(self):
        if self.tick_id is None and not self.closed:
            self.last=time.monotonic()
            self.tick_id=self.area.add_tick_callback(self.tick)

    def tick(self,area,clock):
        if self.closed:
            self.tick_id=None; return False
        now=time.monotonic(); t=now-self.started
        if self.birthing:
            self.model.advance(t,now-self.last)
            if t>=BIRTH_SECONDS:
                self.birthing=False
                self.width,self.height=self.WIDTH,self.HEIGHT
                self.area.set_size_request(self.width,self.height)
                self.window.resize(self.width,self.height); self.window.move(*self.home)
                self.shape()
        self.last=now; self.area.queue_draw()
        active=self.birthing or now-self.pulse_at<.8 or any(now-v<.7 for v in self.new_at.values()) or self.hover
        if self.reduced or not active:
            self.tick_id=None; return False
        return True

    def poll(self):
        if self.closed: self.poll_id=None; return False
        # GDK position is sufficient for native/XWayland; obtain the Wayland cursor
        # asynchronously, with a single in-flight read, so frames never wait on IPC.
        if not getattr(self,'reading_pointer',False):
            self.reading_pointer=True
            def read():
                from .sand_surface import _pointer
                point=_pointer(self.Gdk)
                self.GLib.idle_add(self._pointer_done,point)
            threading.Thread(target=read,daemon=True).start()
        from .records import RecordStore
        try:
            current=RecordStore().journey(self.journey['session_key'])
            if current and (current['target_steps'],current['completed']) != (self.journey['target_steps'],self.journey['completed']):
                self.update(current,self.thoughts)
        except Exception:
            pass  # The desktop reports ledger failures; the courtesy surface keeps drawing.
        if self.audio is not None:
            from .sand_audio import enabled
            if not enabled() or (time.monotonic()-self.started>7 and not self.audio.busy()):
                self.audio.close(); self.audio=None
        return True

    def _pointer_done(self,point):
        self.reading_pointer=False
        if self.closed or point is None or self.birthing: return False
        x,y=self.window.get_position()
        self.mouse=(point[0]-x,point[1]-y)
        near=math.hypot(self.mouse[0]-62,self.mouse[1]-48)<62
        if near != self.hover:
            self.hover=near
            # Only the small visible core is clickable; every transparent pixel passes.
            self.shape(near)
            if near: self.pulse()
            self.area.queue_draw()
        if near and not self.reduced: self.animate()
        return False

    def pulse(self,sound=False):
        if self.closed: return
        self.pulse_at=time.monotonic(); self.animate(); self.area.queue_draw()
        if sound:
            if self.audio:
                self.audio.note(72,.12,.55,soft=True)
                self.audio.pour(.25,12,.3,.6,low=2200,high=3400,gain=.008)
            elif not getattr(self, 'response_loading', False):
                self.response_loading=True
                generation=self.audio_generation
                def tiny():
                    from .sand_audio import Mixer
                    mixer=Mixer.create()
                    self.GLib.idle_add(self._tiny_ready,mixer,generation)
                threading.Thread(target=tiny,daemon=True).start()

    def _tiny_ready(self,mixer,generation):
        self.response_loading=False
        if self.closed or generation!=self.audio_generation:
            if mixer: mixer.close()
            return False
        self.audio=mixer
        if mixer:
            mixer.note(72,.12,.55,soft=True)
            mixer.pour(.25,12,.3,.6,low=2200,high=3400,gain=.008)
        return False

    def update(self,journey,thoughts=()):
        previous_target=self.journey['target_steps']
        old={t['id'] for t in self.thoughts}
        self.journey=journey; self.thoughts=list(thoughts)[-12:]
        now=time.monotonic()
        for thought in self.thoughts:
            if thought['id'] not in old: self.new_at[thought['id']]=now
        self.layout.set_text(progress(journey),-1)
        self.window.set_tooltip_text('Завершённые шаги: '+progress(journey)+'\n'+ '\n\n'.join(t['text'] for t in self.thoughts))
        if old != {t['id'] for t in self.thoughts} or previous_target!=journey['target_steps']: self.pulse()
        self.area.queue_draw()

    def _start_audio(self):
        self.audio_source=None
        if self.closed: return False
        # Reuse the current streaming mixer and assets; no new player/dependency.
        generation=self.audio_generation
        def prepare():
            from .sand_audio import Mixer
            mixer=Mixer.create()
            self.GLib.idle_add(self._audio_ready,mixer,generation)
        threading.Thread(target=prepare,daemon=True).start()
        return False

    def _audio_ready(self,mixer,generation):
        if self.closed or generation!=self.audio_generation:
            if mixer: mixer.close()
            return False
        self.audio=mixer
        if mixer:
            # F Lydian: immediate high C, ignition A/G, settling C. No low drone.
            t=time.monotonic()-self.started
            for when,midi,gain in ((.06,84,.14),(.85,69,.2),(1.05,79,.13),(3.35,72,.13)):
                if when+.18>=t: mixer.note(midi,gain,0 if when<2.4 else .55,max(0,when-t),soft=True)
            target=self.journey.get('target_steps') or 0
            for i in range(max(0,target-1)):
                when=1.75+i*.14
                if when+.18>=t: mixer.note((81,84,86,77)[i],.07,.25,max(0,when-t),soft=True)
            mixer.pour(max(.1,1.5-t),65,-.25,.25,low=1500,high=3800,gain=.012)
            mixer.pour(1.2,40,0,.55,low=1300,high=3000,gain=.008)
        return False

    @staticmethod
    def halo(p,x,y,r,colour,alpha):
        c=p.c
        if not hasattr(c,'cairo_pattern_create_radial_initium'):
            # libcairo is already used by sand_canvas. Declare only gradient calls.
            for name,args,res in (
                ('cairo_pattern_create_radial',[C.c_double]*6,C.c_void_p),
                ('cairo_pattern_add_color_stop_rgba',[C.c_void_p]+[C.c_double]*5,None),
                ('cairo_set_source',[C.c_void_p,C.c_void_p],None),
                ('cairo_pattern_destroy',[C.c_void_p],None)):
                fn=getattr(c,name); fn.argtypes=args; fn.restype=res
            c.cairo_pattern_create_radial_initium=True
        pat=c.cairo_pattern_create_radial(x,y,0,x,y,r)
        for stop,a in ((0,.85),(.08,.65),(.22,.25),(.5,.045),(1,0)):
            c.cairo_pattern_add_color_stop_rgba(pat,stop,*colour,alpha*a)
        c.cairo_set_source(p.cr,pat); c.cairo_arc(p.cr,x,y,r,0,math.tau); c.cairo_fill(p.cr)
        c.cairo_pattern_destroy(pat)

    def paint(self,p):
        if self.closed: return
        self.frames+=1
        if self.first_frame is None: self.first_frame=time.monotonic()-self.started
        p.clear(); now=time.monotonic(); t=now-self.started
        x,y,scale=self.model.pose(t) if self.birthing else (62,48,.62)
        if self.hover and self.mouse and not self.reduced:
            x+=(self.mouse[0]-62)*.035; y+=(self.mouse[1]-48)*.035
        tint=rgb(self.identity['tint'])
        pulse=0 if self.reduced else max(0,1-(now-self.pulse_at)/.75)
        glow=1+.25*math.sin(pulse*math.pi)
        self.halo(p,x,y,74*scale,tint,.65*glow)
        self.halo(p,x,y,22*scale,rgb(theme.DARK['ink']),.8*glow)
        c,cr=p.c,p.cr
        for k in range(self.identity['rays']):
            angle=self.identity['rotation']+k*math.tau/self.identity['rays']
            length=(25 if k%2==0 else 18)*scale
            for i in range(9):
                u=i/9; v=(i+1)/9
                c.cairo_set_source_rgba(cr,*tint,.6*(1-u)**2)
                c.cairo_set_line_width(cr,.7)
                c.cairo_move_to(cr,x+math.cos(angle)*length*u,y+math.sin(angle)*length*u)
                c.cairo_line_to(cr,x+math.cos(angle)*length*v,y+math.sin(angle)*length*v)
                c.cairo_stroke(cr)
        c.cairo_set_source_rgba(cr,1,.98,.93,1)
        c.cairo_arc(cr,x,y,2.4*scale,0,math.tau); c.cairo_fill(cr)
        if self.birthing:
            buckets=[[] for _ in range(8)]
            fade=1-smooth((t-2.5)/1.0)
            for grain in self.model.particles:
                alpha=(.25 if t<grain['delay'] else .55)*fade
                buckets[min(7,int(alpha*8))].append((grain['x'],grain['y'],grain['size']))
            p.grains(buckets,rgb(theme.DARK['ink']),8)
        target=self.journey['target_steps'] or 0
        for i in range(target):
            ang=math.pi*.55+i*math.pi*1.1/max(1,target-1)
            a=(.65 if i<self.journey['completed'] else .15)*(smooth((t-1.7)/.5) if self.birthing else 1)
            p.ring(x+math.cos(ang)*35,y+math.sin(ang)*35,1.35,tint,a,.8)
        # Satellites are actual saved texts. These lines show session membership,
        # never semantic relationships or an assertion that material is mastered.
        for i,thought in enumerate(self.thoughts):
            a=self.identity['rotation']+i*2.39996
            r=23+(i%3)*9
            px=x+math.cos(a)*r; py=y+math.sin(a)*r
            reveal=1 if self.reduced else smooth((now-self.new_at.get(thought['id'],-100))/.6)
            self.halo(p,px,py,8,tint,.4*reveal)
            c.cairo_set_source_rgba(cr,*tint,.8*reveal)
            c.cairo_arc(cr,px,py,1.2,0,math.tau); c.cairo_fill(cr)
        if not self.birthing:
            w,h=self.layout.get_pixel_size()
            # Opaque enough to hold ink at full contrast over any page.
            p.plate(62-w/2-5,87,w+10,h+6,5,rgb(theme.DARK['paper']),.85)
            p.text(self.layout,62-w/2,90,rgb(theme.DARK['ink']),1)

    def close(self):
        if self.closed: return
        self.closed=True; self.audio_generation+=1
        if self.tick_id is not None: self.area.remove_tick_callback(self.tick_id); self.tick_id=None
        for name in ('poll_id','audio_source'):
            source=getattr(self,name)
            if source: self.GLib.source_remove(source); setattr(self,name,None)
        if self.audio: self.audio.close(); self.audio=None
        self.window.destroy()
