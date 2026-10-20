package dev.fpl.canvas;

import android.app.Activity;
import android.content.Intent;
import android.media.AudioManager;
import android.media.MediaPlayer;
import android.net.Uri;
import android.os.Bundle;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.View;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.TextView;
import android.widget.VideoView;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.HorizontalScrollView;
import android.widget.TextClock;
import android.widget.ImageView;
import android.widget.ScrollView;
import android.graphics.Bitmap;
import android.graphics.Paint;
import android.view.SurfaceView;
import android.view.SurfaceHolder;
import android.os.HandlerThread;
import android.opengl.EGL14;
import android.opengl.EGLDisplay;
import android.opengl.EGLContext;
import android.opengl.EGLSurface;
import android.opengl.EGLConfig;
import android.opengl.GLES30;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.Calendar;
import android.graphics.pdf.PdfRenderer;
import android.os.ParcelFileDescriptor;
import java.io.File;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.Future;
import android.os.Handler;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.StateListDrawable;
import android.content.res.ColorStateList;
import org.json.JSONObject;
import org.json.JSONArray;
import java.util.ArrayList;

/** Optional platform-only shim: no discovery, credentials, or workspace policy. */
public final class TvActivity extends Activity {
    private static final boolean TUNER_CONTROL_AVAILABLE;
    static {
        System.loadLibrary("canvas_android");
        boolean available;
        try { System.loadLibrary("canvas_tuner"); available=true; }
        catch(UnsatisfiedLinkError error) { available=false; android.util.Log.i("CanvasTuner","Tuner control feature not packaged"); }
        TUNER_CONTROL_AVAILABLE=available;
    }
    private static native String nativeValidate(String url);
    private static native float nativeSetMuted(boolean muted);
    private static native String nativeAudioUpdate(String surface, String title, String stream, boolean ready, boolean muted);
    private static native boolean nativeAudioAcknowledge(String request, long revision, boolean muted);
    private static native boolean nativeIsMuted();
    private static native float[] nativeObserverFrame(float seconds);
    private static native String nativeObserverState();
    private ObserverView observerView;
    private ControlNoticeView controlNoticeView;
    private static native int[] nativeVideoFrame(int width, int height, boolean windowed);
    private static native String nativeStartControl(String directory);
    private static native String nativeProjection(int width, int height);
    private static native String nativeSelectWorkspace(String project);
    private static native String nativeSelectPage(int index);
    private static native String nativePdfAsset(String asset);
    private static native float[] nativeClockFrame(int width, int height, int hour, int minute, float second);
    private static native String nativeBackgroundProgram(String id);
    private static native float[] nativeBoidsFrame(int width, int height, float elapsed, boolean aquarium);
    static native String nativeSignalSnapshot(String id);
    static native String nativeMapRaster(String map, int width, int height);
    private static native void nativeResetMapMotion();
    private static native void nativeMapManual(String id);
    static native String nativeMapKiosk(String id);
    private static native String nativeTaskNavigate(String id, String action);
    static native float[] nativeSignalFrame(int width, int height, String frame, String appearance);
    static native String nativeSignalLabels(int width, int height, String frame);
    private static native String nativeTunerControl(String base, String channel, String surface, String directory);
    private String tunerEndpoint = "", videoSurfaceId = "";
    private Button channelButton;
    private boolean tunerBusy;
    private LinearLayout channelScreen;
    private String channelScreenSurface = "";
    private String currentWorkspaceId = "", channelScreenWorkspace = "";
    private String currentProjectId = "", previousProjectId = "";
    private boolean tunerWorkspace() {
        return "television".equals(currentProjectId) && !tunerEndpoint.isEmpty();
    }
    private GpuBackground background;
    private ImageView themeWallpaper;
    private VideoView video;
    private TextView status;
    private FrameLayout frame;
    private Button muteButton;
    private MediaPlayer preparedPlayer;
    private final java.util.Map<String, StreamTileView> extraStreams = new java.util.HashMap<>();
    private final PageRotation pageRotation = new PageRotation();
    private int rotationInterval = 5000;
    private final java.util.Map<String, View> panelCache = new java.util.HashMap<>();
    private String renderedWorkspace = "";
    private int renderedPage = -1;
    private void clearPanelCache() {
        for (View panel : panelCache.values()) {
            panel.animate().cancel();
            if (panel instanceof PdfSurface) ((PdfSurface)panel).dispose();
            frame.removeView(panel);
        }
        panelCache.clear();
    }
    private String panelKey(JSONObject tile, JSONObject palette) throws Exception {
        JSONObject content = tile.getJSONObject("surface").getJSONObject("content");
        boolean pdf = content.getString("kind").equals("asset") && content.getJSONObject("asset").getString("media_type").equals("application/pdf");
        boolean card = content.getString("kind").equals("widget") && content.getJSONObject("spec").getString("kind").equals("rich_card");
        if (card) {
            JSONObject stable = new JSONObject(tile.toString());
            stripStatusValues(stable.getJSONObject("surface"));
            return stable.toString() + palette.toString();
        }
        return pdf ? tile.toString() + palette.toString() : "";
    }
    private void disposeExtraStreams() {
        for (StreamTileView tile : extraStreams.values()) { tile.dispose(); frame.removeView(tile); }
        extraStreams.clear();
    }
    private boolean resumed;
    private LinearLayout selector;
    private final Handler updates = new Handler();
    private String lastProjection = "";
    private String lastStructure = "";
    private String currentUrl = "";
    private String surfaceTitle = "KOMO";
    private boolean workspaceMode;
    private TextView workspaceHeader;
    private LibraryView libraryScreen;
    private LibraryView cachedLibraryScreen;
    private String libraryEndpoint = "";
    private String consoleAppearance = "";
    private View libraryReturnFocus;
    private int libraryReturnFocusability;
    private boolean pendingLibraryOpen;
    private int libraryMuteVisibility, libraryChannelVisibility, libraryStatusVisibility;

    private void openRequestedLibrary() {
        if(!pendingLibraryOpen || frame.getWidth()==0 || frame.getHeight()==0)return;
        pendingLibraryOpen=false;
        showLibrary();
    }

    private void showLibrary() {
        if (libraryScreen != null || libraryEndpoint.isEmpty()) return;
        try {
            JSONObject palette = new JSONObject(nativeProjection(frame.getWidth(),frame.getHeight())).getJSONObject("palette");
            libraryReturnFocus = getCurrentFocus();
            libraryReturnFocusability = selector.getDescendantFocusability();
            if(cachedLibraryScreen==null)cachedLibraryScreen = new LibraryView(this,libraryEndpoint,palette,() -> closeLibrary());
            libraryScreen = cachedLibraryScreen;
            libraryMuteVisibility=muteButton.getVisibility();
            libraryChannelVisibility=channelButton.getVisibility();
            libraryStatusVisibility=status.getVisibility();
            muteButton.setVisibility(View.GONE);channelButton.setVisibility(View.GONE);status.setVisibility(View.GONE);
            // An opaque view covers video without destroying its Surface/decoder.
            background.setActive(false);
            selector.setDescendantFocusability(android.view.ViewGroup.FOCUS_BLOCK_DESCENDANTS);
            selector.setFocusable(false);
            FrameLayout.LayoutParams bounds = new FrameLayout.LayoutParams(-1,-1);
            bounds.topMargin = barHeight();
            frame.addView(libraryScreen,bounds);
            libraryScreen.bringToFront();
            libraryScreen.enter();
        } catch(Exception e) { showStatus("Library unavailable: "+e.getMessage()); }
    }

    private void closeLibrary() {
        if(libraryScreen == null) return;
        libraryScreen.leave(); frame.removeView(libraryScreen); libraryScreen=null;
        muteButton.setVisibility(libraryMuteVisibility);channelButton.setVisibility(libraryChannelVisibility);status.setVisibility(libraryStatusVisibility);
        selector.setDescendantFocusability(libraryReturnFocusability);
        background.setActive(resumed);
        for(View widget:widgets) {
            widget.invalidate();
            if(widget instanceof android.view.ViewGroup) {
                android.view.ViewGroup group=(android.view.ViewGroup)widget;
                for(int i=0;i<group.getChildCount();i++)group.getChildAt(i).invalidate();
            }
        }
        if(libraryReturnFocus!=null && libraryReturnFocus.isAttachedToWindow())libraryReturnFocus.requestFocus();
    }
    private HorizontalScrollView workspaceSelector;
    private final ArrayList<View> widgets = new ArrayList<>();
    private final java.util.HashMap<String, StatusListView> statusLists = new java.util.HashMap<>();
    private final java.util.HashMap<String, RichCardView> richCards = new java.util.HashMap<>();
    private final ThreadPoolExecutor pdfJobs = new ThreadPoolExecutor(1, 1, 30, TimeUnit.SECONDS,
        new ArrayBlockingQueue<Runnable>(8));

    /** Independent 720p EGL surface; no CPU readback or 4K UI texture copy. */
    private final class GpuBackground extends SurfaceView implements SurfaceHolder.Callback {
        private final HandlerThread worker = new HandlerThread("canvas-background-gpu");
        private final Handler gpu;
        private EGLDisplay display = EGL14.EGL_NO_DISPLAY;
        private EGLContext context = EGL14.EGL_NO_CONTEXT;
        private EGLSurface output = EGL14.EGL_NO_SURFACE;
        private int program, buffer, width, height;
        private volatile boolean active;
        private String key = "", vertex = "", fragment = "";
        private float speed;
        private final float[] theme = new float[4];
        private final ByteBuffer uniforms = ByteBuffer.allocateDirect(48).order(ByteOrder.nativeOrder());
        private final long epoch = System.nanoTime();
        private long completedFrames, workNanos, sampleStart = System.nanoTime();
        GpuBackground() {
            super(TvActivity.this);
            worker.start(); gpu = new Handler(worker.getLooper());
            setFocusable(false); getHolder().addCallback(this);
            setVisibility(View.GONE);
        }
        private void failure(Exception error) {
            active = false;
            android.util.Log.e("CanvasBackground", "GPU background failed", error);
            runOnUiThread(() -> { setVisibility(View.GONE); showStatus("GPU background: " + error.getMessage()); });
        }
        void configure(JSONObject document, JSONObject palette) throws Exception {
            JSONObject spec = document.optJSONObject("background");
            if (spec == null || !spec.optString("kind").equals("shader")) {
                active = false; setVisibility(View.GONE); gpu.removeCallbacks(draw); key = "";
                gpu.post(this::releaseProgram);
                return;
            }
            String next = spec.toString() + palette.getString("background");
            if (next.equals(key)) return;
            JSONObject source = new JSONObject(nativeBackgroundProgram(spec.getString("shader_id")));
            if (source.has("error")) throw new IllegalArgumentException(source.getString("error"));
            float rate = (float)spec.getDouble("speed");
            if (!Float.isFinite(rate) || rate < 0 || rate > 4) throw new IllegalArgumentException("Invalid shader speed");
            int color = Color.parseColor(palette.getString("background"));
            final String vs = source.getString("vertex"), fs = source.getString("fragment");
            key = next; setVisibility(View.VISIBLE); active = resumed;
            gpu.post(() -> {
                vertex = vs; fragment = fs; speed = rate;
                theme[0] = Color.red(color) / 255f; theme[1] = Color.green(color) / 255f;
                theme[2] = Color.blue(color) / 255f; theme[3] = 1;
                try { if (context != EGL14.EGL_NO_CONTEXT) installProgram(); schedule(); }
                catch (Exception error) { failure(error); }
            });
        }
        void setActive(boolean visible) {
            active = visible && getVisibility() == View.VISIBLE;
            gpu.removeCallbacks(draw);
            if (active) gpu.post(() -> {
                completedFrames = 0; workNanos = 0; sampleStart = System.nanoTime(); schedule();
            });
        }
        private int shader(int kind, String code) {
            int shader = GLES30.glCreateShader(kind);
            GLES30.glShaderSource(shader, code); GLES30.glCompileShader(shader);
            int[] ok = new int[1]; GLES30.glGetShaderiv(shader, GLES30.GL_COMPILE_STATUS, ok, 0);
            if (ok[0] == 0) {
                String reason = GLES30.glGetShaderInfoLog(shader); GLES30.glDeleteShader(shader);
                throw new IllegalStateException(reason);
            }
            return shader;
        }
        private void releaseProgram() {
            if (program != 0) GLES30.glDeleteProgram(program);
            if (buffer != 0) GLES30.glDeleteBuffers(1, new int[]{buffer}, 0);
            program = 0; buffer = 0;
        }
        private void installProgram() {
            releaseProgram();
            int vs = shader(GLES30.GL_VERTEX_SHADER, vertex), fs = 0;
            try {
                fs = shader(GLES30.GL_FRAGMENT_SHADER, fragment);
                program = GLES30.glCreateProgram(); GLES30.glAttachShader(program, vs); GLES30.glAttachShader(program, fs);
                GLES30.glLinkProgram(program);
                int[] ok = new int[1]; GLES30.glGetProgramiv(program, GLES30.GL_LINK_STATUS, ok, 0);
                if (ok[0] == 0) throw new IllegalStateException(GLES30.glGetProgramInfoLog(program));
                int[] buffers = new int[1]; GLES30.glGenBuffers(1, buffers, 0); buffer = buffers[0];
                GLES30.glBindBuffer(GLES30.GL_UNIFORM_BUFFER, buffer);
                GLES30.glBufferData(GLES30.GL_UNIFORM_BUFFER, 48, null, GLES30.GL_DYNAMIC_DRAW);
                GLES30.glBindBufferBase(GLES30.GL_UNIFORM_BUFFER, 0, buffer);
            } finally { GLES30.glDeleteShader(vs); if (fs != 0) GLES30.glDeleteShader(fs); }
        }
        private void schedule() { gpu.removeCallbacks(draw); if (active) gpu.post(draw); }
        private final Runnable draw = new Runnable() {
            @Override public void run() {
                if (!active || program == 0 || output == EGL14.EGL_NO_SURFACE) return;
                try {
                    long started = System.nanoTime();
                    uniforms.clear(); uniforms.putFloat(width).putFloat(height);
                    uniforms.putFloat(speed == 0 ? 0 : (System.nanoTime() - epoch) / 1e9f * speed).putFloat(0);
                    for (int i = 0; i < 4; i++) uniforms.putFloat(0);
                    for (float value : theme) uniforms.putFloat(value);
                    uniforms.flip();
                    GLES30.glViewport(0, 0, width, height); GLES30.glUseProgram(program);
                    GLES30.glBindBuffer(GLES30.GL_UNIFORM_BUFFER, buffer);
                    GLES30.glBufferSubData(GLES30.GL_UNIFORM_BUFFER, 0, 48, uniforms);
                    GLES30.glDrawArrays(GLES30.GL_TRIANGLES, 0, 3);
                    int error = GLES30.glGetError();
                    if (error != GLES30.GL_NO_ERROR) throw new IllegalStateException("GL error " + error);
                    if (!EGL14.eglSwapBuffers(display, output)) throw new IllegalStateException("EGL swap failed");
                    // One submitted frame at a time; wait for completion before pacing.
                    GLES30.glFinish();
                    workNanos += System.nanoTime() - started;
                    if (++completedFrames == 300) {
                        long now = System.nanoTime();
                        android.util.Log.i("CanvasBackground", "fps=" + (completedFrames * 1e9 / (now - sampleStart))
                            + " mean-frame-work-ms=" + (workNanos / 1e6 / completedFrames));
                        completedFrames = 0; workNanos = 0; sampleStart = now;
                    }
                    if (active && speed > 0) gpu.postDelayed(this,
                        Math.max(1, 33 - TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started)));
                } catch (Exception error) { failure(error); }
            }
        };
        @Override public void surfaceCreated(SurfaceHolder holder) {
            int w = Math.max(1, getWidth()), h = Math.max(1, getHeight());
            float scale = Math.min(1, Math.min(1280f / w, 720f / h));
            width = Math.max(1, Math.round(w * scale)); height = Math.max(1, Math.round(h * scale));
            holder.setFixedSize(width, height);
            gpu.post(() -> {
                try {
                    display = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY);
                    if (!EGL14.eglInitialize(display, new int[2], 0, new int[2], 0)) throw new IllegalStateException("EGL initialize failed");
                    EGLConfig[] configs = new EGLConfig[1]; int[] count = new int[1];
                    int[] attributes = {EGL14.EGL_RENDERABLE_TYPE, 0x40, EGL14.EGL_SURFACE_TYPE, EGL14.EGL_WINDOW_BIT,
                        EGL14.EGL_RED_SIZE, 8, EGL14.EGL_GREEN_SIZE, 8, EGL14.EGL_BLUE_SIZE, 8, EGL14.EGL_ALPHA_SIZE, 8, EGL14.EGL_NONE};
                    if (!EGL14.eglChooseConfig(display, attributes, 0, configs, 0, 1, count, 0) || count[0] == 0)
                        throw new IllegalStateException("No GLES3 window config");
                    context = EGL14.eglCreateContext(display, configs[0], EGL14.EGL_NO_CONTEXT,
                        new int[]{EGL14.EGL_CONTEXT_CLIENT_VERSION, 3, EGL14.EGL_NONE}, 0);
                    output = EGL14.eglCreateWindowSurface(display, configs[0], holder.getSurface(), new int[]{EGL14.EGL_NONE}, 0);
                    if (!EGL14.eglMakeCurrent(display, output, output, context)) throw new IllegalStateException("EGL context failed");
                    completedFrames = 0; workNanos = 0; sampleStart = System.nanoTime();
                    android.util.Log.i("CanvasBackground", "GPU=" + GLES30.glGetString(GLES30.GL_RENDERER)
                        + " GL=" + GLES30.glGetString(GLES30.GL_VERSION) + " buffer=" + width + "x" + height);
                    if (!vertex.isEmpty()) installProgram(); schedule();
                } catch (Exception error) { failure(error); }
            });
        }
        private void release() {
            gpu.removeCallbacks(draw); releaseProgram();
            if (display != EGL14.EGL_NO_DISPLAY) {
                EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT);
                if (output != EGL14.EGL_NO_SURFACE) EGL14.eglDestroySurface(display, output);
                if (context != EGL14.EGL_NO_CONTEXT) EGL14.eglDestroyContext(display, context);
                EGL14.eglTerminate(display);
            }
            display = EGL14.EGL_NO_DISPLAY; context = EGL14.EGL_NO_CONTEXT; output = EGL14.EGL_NO_SURFACE;
        }
        @Override public void surfaceDestroyed(SurfaceHolder holder) {
            gpu.removeCallbacks(draw);
            java.util.concurrent.CountDownLatch released = new java.util.concurrent.CountDownLatch(1);
            if (gpu.post(() -> { release(); released.countDown(); })) {
                try {
                    if (!released.await(2, TimeUnit.SECONDS)) failure(new IllegalStateException("GPU teardown timed out"));
                } catch (InterruptedException error) { Thread.currentThread().interrupt(); failure(error); }
            }
        }
        @Override public void surfaceChanged(SurfaceHolder holder, int format, int w, int h) {
            gpu.post(() -> { width = w; height = h; schedule(); });
        }
        void dispose() { active = false; gpu.post(() -> { release(); worker.quitSafely(); }); }
    }

    /** Paints canonical gfx primitives; no duplicate clock geometry in Java. */
    /** Platform adapter for the same renderer-neutral chrome as the native Mac popup. */
    private final class ObserverView extends View {
        final Paint ink=new Paint(Paint.ANTI_ALIAS_FLAG);
        final android.graphics.RectF arc=new android.graphics.RectF();
        final long started=System.nanoTime();
        int[] colors={0xff101820,0xff55ccee,0xff051719};
        JSONObject chrome;
        String caption="";
        float progress=0;
        long lastDraw=0, hideAt=0;
        boolean active=false;
        float[] retainedRecords=new float[0];
        void palette(JSONObject palette) { /* Voice chrome comes from shared GFX, not workspace theme. */ }
        int rgba(org.json.JSONArray rgba) throws Exception {
            return Color.argb(rgba.length()>3 ? Math.round((float)rgba.getDouble(3)*255) : 255,
                Math.round((float)rgba.getDouble(0)*255), Math.round((float)rgba.getDouble(1)*255),
                Math.round((float)rgba.getDouble(2)*255));
        }
        float metric(String name) throws Exception { return (float)chrome.getDouble(name); }
        ObserverView(){super(TvActivity.this);setVisibility(View.GONE);setFocusable(false);setClickable(false);}
        void refresh(){
            String raw=nativeObserverState();
            try{
                long now=android.os.SystemClock.elapsedRealtime();
                if(raw==null || raw.equals("null")){
                    // Expiry/disconnect dismisses immediately, without claiming idle success.
                    active=false;hideAt=0;
                } else {
                    JSONObject state=new JSONObject(raw);
                    JSONObject next=state.getJSONObject("popup");
                    boolean nextActive=!state.getString("activity").equals("hidden");
                    if(active && !nextActive)hideAt=now+Math.round(next.getDouble("linger_seconds")*1000);
                    if(nextActive)hideAt=0;
                    // Keep the final heard line while the idle readback fades.
                    if(nextActive || active || progress==0){
                        chrome=next;
                        if(nextActive || progress==0 || (!state.isNull("caption") && !state.getString("caption").isEmpty()))caption=next.getString("label");
                    }
                    active=nextActive;
                    colors=new int[]{rgba(next.getJSONArray("surface")),rgba(next.getJSONArray("accent")),rgba(next.getJSONArray("ink"))};
                }
                if(chrome!=null && (active || hideAt>now || progress>0)){
                    setContentDescription(caption);setVisibility(View.VISIBLE);invalidate();
                }
            }catch(Exception error){active=false;hideAt=0;invalidate();android.util.Log.e("CanvasObserver","Invalid observer state",error);}
        }
        @Override protected void onDraw(android.graphics.Canvas target){
            if(!resumed || chrome==null)return;
            try {
                long now=android.os.SystemClock.elapsedRealtime();
                float dt=lastDraw==0 ? 1f/60 : Math.min(.05f,(now-lastDraw)/1000f);lastDraw=now;
                boolean visible=active || now<hideAt;
                progress=visible ? Math.min(1,progress+dt/metric("reveal_seconds")) : Math.max(0,progress-dt/metric("dismiss_seconds"));
                if(progress==0){setVisibility(View.GONE);lastDraw=0;return;}
                float opacity=1-(float)Math.pow(1-progress,3);
                float[] records=nativeObserverFrame((System.nanoTime()-started)/1000000000f);
                if(records==null || records.length%9!=0 || records.length>38*9)throw new IllegalStateException("Invalid reactor records");
                if(records.length>0)retainedRecords=records;else records=retainedRecords;
                org.json.JSONArray size=chrome.getJSONArray("size");
                float width=(float)size.getDouble(0),height=(float)size.getDouble(1);
                float scale=getWidth()/width;
                target.save();target.scale(scale,scale);
                target.translate(0,metric("slide")*(1-opacity));
                int layer=target.saveLayerAlpha(0,0,width,height,Math.round(255*opacity));
                ink.setColor(rgba(chrome.getJSONArray("surface")));ink.setStyle(Paint.Style.FILL);
                arc.set(0,0,width,height);target.drawRoundRect(arc,metric("radius"),metric("radius"),ink);
                ink.setColor(rgba(chrome.getJSONArray("border")));ink.setStyle(Paint.Style.STROKE);ink.setStrokeWidth(1);
                target.drawRoundRect(arc,metric("radius"),metric("radius"),ink);
                ink.setColor(rgba(chrome.getJSONArray("text")));ink.setStyle(Paint.Style.FILL);
                ink.setTypeface(android.graphics.Typeface.create("sans-serif",android.graphics.Typeface.NORMAL));
                ink.setTextSize(metric("font_size"));
                org.json.JSONArray text=chrome.getJSONArray("text_rect");
                target.save();target.clipRect((float)text.getDouble(0),0,(float)(text.getDouble(0)+text.getDouble(2)),height);
                Paint.FontMetrics fm=ink.getFontMetrics();
                target.drawText(caption,(float)text.getDouble(0),height/2-(fm.ascent+fm.descent)/2,ink);target.restore();
                org.json.JSONArray center=chrome.getJSONArray("reactor_center");
                float reactorScale=metric("reactor_width")/176f;
                target.translate((float)center.getDouble(0)-88*reactorScale,(float)center.getDouble(1)-83*reactorScale);
                target.scale(reactorScale,reactorScale);
                for(int i=0;i<records.length;i+=9){
                    float x=records[i+1],y=records[i+2],r=records[i+3];
                    ink.setColor(colors[(int)records[i+7]]);ink.setAlpha(Math.round(records[i+8]*255));
                    ink.setStyle(records[i]==2 || records[i]==3 ? Paint.Style.STROKE : Paint.Style.FILL);ink.setStrokeWidth(records[i+4]);
                    if(records[i]==1 || records[i]==2)target.drawCircle(x,y,r,ink);
                    else if(records[i]==3){arc.set(x-r,y-r,x+r,y+r);target.drawArc(arc,records[i+5],records[i+6],false,ink);}
                    else if(records[i]==4)target.drawRect(x,y,x+r,y+records[i+4],ink);
                    else throw new IllegalStateException("Unsupported reactor mark");
                }
                target.restoreToCount(layer);target.restore();postInvalidateOnAnimation();
            }catch(Exception error){setVisibility(View.GONE);android.util.Log.e("CanvasObserver","Invalid popup frame",error);}
        }
    }

    /** Exact player target; only native owner pending/ack state supplies notices. */
    private final class ControlNoticeView extends View {
        final Paint ink = new Paint(Paint.ANTI_ALIAS_FLAG);
        String state = "";
        long received;
        ControlNoticeView() { super(TvActivity.this); setFocusable(false); setClickable(false); }
        void update(JSONObject snapshot) throws Exception {
            JSONObject notice = snapshot.optJSONObject("control_notice");
            state = notice == null ? "" : notice.getString("state");
            received = android.os.SystemClock.elapsedRealtime();
            setContentDescription(state.isEmpty() ? "" : "Audio control " + state);
            invalidate();
        }
        @Override protected void onDraw(android.graphics.Canvas target) {
            if (!resumed || state.isEmpty() || state.equals("confirmed") || video.getVisibility() != View.VISIBLE
                || android.os.SystemClock.elapsedRealtime()-received >= 500) return;
            ink.setColor(observerView.colors[1]);
            ink.setStyle(Paint.Style.FILL); ink.setTextSize(dp(12));
            target.drawText("Audio pending…",
                video.getLeft()+dp(12),video.getBottom()-dp(12),ink);
            postInvalidateDelayed(250);
        }
    }

    /** Paints canonical gfx primitives; no duplicate clock geometry in Java. */
    /** Local, bounded Rust flock rendered with hardware-accelerated UI strokes. */
    private final class BoidsView extends View {
        private final String footnote;
        private final android.text.TextPaint noteInk = new android.text.TextPaint(Paint.ANTI_ALIAS_FLAG);
        private final Paint notePanel = new Paint();
        private android.text.StaticLayout noteLayout;
        private boolean showNote = true;
        private final boolean aquarium;
        private final int fishInk, sharkInk;
        private final Paint ink = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final long epoch = System.nanoTime();
        private long sample = epoch, work;
        private int frames;
        BoidsView(JSONObject palette, boolean aquarium, String footnote) throws Exception {
            super(TvActivity.this);
            if (footnote.length() > 2048) throw new IllegalArgumentException("Footnote exceeds limit");
            this.footnote = footnote;
            noteInk.setTextSize(dp(18)); noteInk.setColor(Color.parseColor(palette.getString("text_muted")));
            notePanel.setColor(Color.parseColor(palette.getString("surface")));
            this.aquarium = aquarium;
            fishInk = Color.parseColor(palette.getString("primary"));
            sharkInk = Color.parseColor(palette.getString("text"));
            setBackgroundColor(aquarium ? Color.rgb(2,18,35) : Color.BLACK);
            setContentDescription("Boids · local flock");
            ink.setColor(Color.parseColor(palette.getString("primary")));
            ink.setStyle(Paint.Style.STROKE); ink.setStrokeCap(Paint.Cap.ROUND);
        }
        boolean toggleFootnote() {
            if (footnote.isEmpty()) return false;
            showNote = !showNote; invalidate(); return true;
        }
        @Override protected void onSizeChanged(int w, int h, int oldw, int oldh) {
            super.onSizeChanged(w,h,oldw,oldh);
            if (!footnote.isEmpty()) noteLayout = new android.text.StaticLayout(
                footnote,noteInk,Math.max(1,Math.min(dp(560),(int)(w*0.36f)-dp(32))),android.text.Layout.Alignment.ALIGN_NORMAL,1.15f,0f,false);
        }
        @Override protected void onDraw(android.graphics.Canvas target) {
            super.onDraw(target);
            if (!resumed || getWidth() == 0 || getHeight() == 0) return;
            long start = System.nanoTime();
            float elapsed=(start-epoch)/1000000000f;
            if (aquarium) {
                ink.setColor(Color.rgb(12,49,64)); ink.setStrokeWidth(2f);
                for (int b=0;b<18;b++) {
                    float x=(b*0.061f+0.035f)*getWidth();
                    float y=(1f-((elapsed*0.023f+b*0.137f)%1f))*getHeight();
                    target.drawCircle(x,y,3f+(b%4)*2f,ink);
                }
            }
            float[] records = nativeBoidsFrame(getWidth(), getHeight(), elapsed, aquarium);
            if (records == null || records.length % 8 != 0) throw new IllegalStateException("Invalid flock primitives");
            for (int i=0; i<records.length; i+=8) {
                if(records[i] == 9f) { target.drawARGB(Math.round(records[i+1]*110f),160,0,20); continue; }
                if(records[i] != 5f) throw new IllegalStateException("Unsupported flock primitive");
                ink.setStrokeWidth(records[i+5]);
                ink.setColor(records[i+6] == 2f ? sharkInk : fishInk);
                target.drawLine(records[i+1],records[i+2],records[i+3],records[i+4],ink);
            }
            if (!footnote.isEmpty()) {
                float bottom=getHeight()-dp(20);
                if(showNote && noteLayout != null) {
                    float left=getWidth()-noteLayout.getWidth()-dp(48);
                    float top=Math.max(dp(8),bottom-noteLayout.getHeight()-dp(44));
                    target.drawRect(left,top,getWidth()-dp(16),getHeight()-dp(12),notePanel);
                    target.save(); target.clipRect(left,top,getWidth()-dp(16),getHeight()-dp(12));
                    target.translate(left+dp(16),top+dp(14)); noteLayout.draw(target); target.restore();
                }
                target.drawText("Play/Pause · rules",getWidth()-dp(32)-noteInk.measureText("Play/Pause · rules"),bottom,noteInk);
            }
            frames++; work += System.nanoTime()-start;
            if (start-sample >= 10000000000L) {
                android.util.Log.i("CanvasBoids","fps="+(frames*1000000000.0/(start-sample))+" mean-cpu-frame-work-ms="+(work/1000000.0/frames));
                frames=0;work=0;sample=start;
            }
            postInvalidateOnAnimation();
        }
    }

    private final class AnalogClockView extends View {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Calendar time = Calendar.getInstance();
        private final int[] inks;
        AnalogClockView(JSONObject palette) throws Exception {
            super(TvActivity.this);
            inks = new int[]{Color.parseColor(palette.getString("surface")),
                Color.parseColor(palette.getString("text_muted")),
                Color.parseColor(palette.getString("text")),
                Color.parseColor(palette.getString("primary"))};
            setBackgroundColor(Color.TRANSPARENT);
            setContentDescription("Analog clock");
            paint.setStrokeCap(Paint.Cap.ROUND);
        }
        @Override protected void onDraw(android.graphics.Canvas target) {
            super.onDraw(target);
            if (!resumed || getWidth() == 0 || getHeight() == 0) return;
            time.setTimeInMillis(System.currentTimeMillis());
            float density = getResources().getDisplayMetrics().density;
            target.save(); target.scale(density, density);
            float[] records = nativeClockFrame(Math.max(1, Math.round(getWidth() / density)), Math.max(1, Math.round(getHeight() / density)), time.get(Calendar.HOUR_OF_DAY),
                time.get(Calendar.MINUTE), time.get(Calendar.SECOND) + time.get(Calendar.MILLISECOND) / 1000f);
            if (records == null || records.length != 65 * 8) throw new IllegalStateException("Invalid clock primitives");
            for (int i = 0; i < records.length; i += 8) {
                paint.setColor(inks[(int)records[i + 6]]);
                if (records[i] == 1f) {
                    paint.setStyle(Paint.Style.FILL);
                    target.drawCircle(records[i + 1], records[i + 2], records[i + 3], paint);
                    if (records[i + 5] > 0) {
                        paint.setStyle(Paint.Style.STROKE);
                        paint.setStrokeWidth(records[i + 5]);
                        paint.setColor(inks[(int)records[i + 7]]);
                        target.drawCircle(records[i + 1], records[i + 2], records[i + 3], paint);
                    }
                } else if (records[i] == 5f) {
                    paint.setStyle(Paint.Style.STROKE); paint.setStrokeWidth(records[i + 5]);
                    target.drawLine(records[i + 1], records[i + 2], records[i + 3], records[i + 4], paint);
                } else throw new IllegalStateException("Unsupported clock primitive");
            }
            target.restore();
            postInvalidateDelayed(40);
        }
    }


    /** Platform raster adapter only; source identity and geometry come from Rust. */
    private final class PdfSurface extends LinearLayout {
        private final PdfPageView image;
        private final FrameLayout scroll;
        private final LinearLayout controls;
        private final TextView label;
        private final Button previous, next, sizing, fitSheet, fitDrawing;
        private final String asset;
        private final String title;
        private final int rasterWidth;
        private int pageIndex, pageCount, generation;
        private int failedPage = -1;
        private boolean disposed, busy;
        private Bitmap bitmap;
        private Future<?> pending;

        PdfSurface(JSONObject source, String title, JSONObject palette, int width, int height) throws Exception {
            super(TvActivity.this);
            this.asset = source.toString(); this.title = title;
            rasterWidth = Math.max(1, Math.min(width, 1920));
            setOrientation(VERTICAL);
            // The page is opaque paper; unused fit/scroll bounds are not chrome.
            setBackgroundColor(Color.TRANSPARENT);
            image = new PdfPageView(TvActivity.this, Color.parseColor(palette.getString("surface")),
                Color.parseColor(palette.getString("text")), () -> updateZoomLabel());
            scroll = new FrameLayout(TvActivity.this);
            scroll.addView(image, new FrameLayout.LayoutParams(-1, -1));
            addView(scroll, new LinearLayout.LayoutParams(-1, 0, 1));
            controls = new LinearLayout(TvActivity.this);
            controls.setGravity(Gravity.CENTER_VERTICAL);
            previous = new Button(TvActivity.this); previous.setText("Previous");
            next = new Button(TvActivity.this); next.setText("Next");
            fitSheet = new Button(TvActivity.this); fitSheet.setText("Fit sheet");
            fitDrawing = new Button(TvActivity.this); fitDrawing.setText("Fit drawing");
            sizing = new Button(TvActivity.this); sizing.setText("Zoom");
            styleControl(previous, palette, false); styleControl(next, palette, false);
            styleControl(sizing, palette, false);
            styleControl(fitSheet, palette, false); styleControl(fitDrawing, palette, false);
            for (Button control : new Button[]{previous,next,sizing,fitSheet,fitDrawing}) control.setTextSize(18);
            label = new TextView(TvActivity.this); label.setTextSize(16);
            label.setGravity(Gravity.CENTER);
            label.setTextColor(Color.parseColor(palette.getString("text")));
            controls.addView(previous, new LinearLayout.LayoutParams(dp(112), -1));
            controls.addView(label, new LinearLayout.LayoutParams(0, -1, 1));
            controls.addView(fitSheet, new LinearLayout.LayoutParams(dp(128), -1));
            controls.addView(fitDrawing, new LinearLayout.LayoutParams(dp(148), -1));
            controls.addView(sizing, new LinearLayout.LayoutParams(dp(104), -1));
            controls.addView(next, new LinearLayout.LayoutParams(dp(96), -1));
            addView(controls, new LinearLayout.LayoutParams(-1, dp(56)));
            previous.setOnClickListener(v -> render(pageIndex - 1));
            next.setOnClickListener(v -> render(pageIndex + 1));
            sizing.setOnClickListener(v -> toggleSizing());
            fitSheet.setOnClickListener(v -> { image.fit(); image.requestFocus(); });
            fitDrawing.setOnClickListener(v -> { image.fitDrawing(); image.requestFocus(); });
            render(0);
        }

        private void toggleSizing() {
            if (failedPage >= 0) render(failedPage); else image.zoom();
            image.requestFocus();
        }

        private void updateZoomLabel() {
            if (sizing == null) return;
            if (failedPage >= 0) { render(failedPage); return; }
            sizing.setText("Zoom");
            label.setText(title + " · " + (pageIndex + 1) + " / " + pageCount
                + " · " + image.mode() + "\nOK zoom | arrows pan | Rewind / Fast-forward sheet | BACK fit / return");
        }

        private void applyPageBounds(Bitmap page, double[] bounds) {
            image.page(page, bounds);
            fitDrawing.setEnabled(bounds != null);
        }

        boolean remote(int key, KeyEvent event) {
            if (failedPage >= 0 && (key == KeyEvent.KEYCODE_DPAD_CENTER || key == KeyEvent.KEYCODE_ENTER)) {
                if (event.getRepeatCount() == 0) render(failedPage);
                return true;
            }
            if (key == KeyEvent.KEYCODE_MEDIA_FAST_FORWARD || key == KeyEvent.KEYCODE_MEDIA_REWIND) {
                if (event.getRepeatCount() == 0) render(pageIndex + (key == KeyEvent.KEYCODE_MEDIA_FAST_FORWARD ? 1 : -1));
                return true;
            }
            if (key == KeyEvent.KEYCODE_BACK && !previousProjectId.isEmpty()) {
                if (event.getRepeatCount() == 0) {
                    if (image.enlarged() && failedPage < 0) image.fit();
                    else {
                        String error = nativeSelectWorkspace(previousProjectId);
                        if (error == null || !error.isEmpty()) showStatus("Return failed: " + error);
                        else { lastProjection=""; refreshWorkspace(); }
                    }
                }
                return true;
            }
            return false;
        }

        private void render(int requested) {
            if (disposed || busy || requested < 0 || (pageCount > 0 && requested >= pageCount)) return;
            busy = true;
            final int ticket = ++generation;
            label.setText(title + " · Loading…");
            try {
                pending = pdfJobs.submit(() -> {
                    Bitmap result = null;
                    try {
                        JSONObject verified = new JSONObject(nativePdfAsset(asset));
                        if (verified.has("error")) throw new IllegalArgumentException(verified.getString("error"));
                        try (ParcelFileDescriptor file = ParcelFileDescriptor.open(new File(verified.getString("path")), ParcelFileDescriptor.MODE_READ_ONLY);
                             PdfRenderer renderer = new PdfRenderer(file)) {
                            final int count = renderer.getPageCount();
                            if (requested >= count) throw new IllegalArgumentException("Page out of range");
                            double[] bounds=null;
                            if (!verified.isNull("viewports") && verified.has("viewports")) {
                                JSONArray regions=verified.getJSONObject("viewports").getJSONArray("pages");
                                if(regions.length()!=count)throw new IllegalArgumentException("Drawing bounds page count mismatch");
                                JSONArray rectangle=regions.getJSONObject(requested).getJSONArray("bbox");
                                bounds=new double[]{rectangle.getDouble(0),rectangle.getDouble(1),rectangle.getDouble(2),rectangle.getDouble(3)};
                            }
                            final double[] drawingBounds=bounds;
                            try (PdfRenderer.Page page = renderer.openPage(requested)) {
                                float scale = Math.min((float)rasterWidth / page.getWidth(), 2560f / page.getHeight());
                                result = Bitmap.createBitmap(Math.max(1, (int)(page.getWidth() * scale)),
                                    Math.max(1, (int)(page.getHeight() * scale)), Bitmap.Config.ARGB_8888);
                                result.eraseColor(Color.WHITE);
                                page.render(result, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY);
                            }
                            final Bitmap ready = result;
                            runOnUiThread(() -> {
                                if (disposed || generation != ticket) { ready.recycle(); return; }
                                controls.setVisibility(count == 1 ? View.GONE : View.VISIBLE);
                                applyPageBounds(ready,drawingBounds);
                                // The viewport holds this single verified raster.
                                if (bitmap != null) bitmap.recycle();
                                bitmap = ready; pageIndex = requested; pageCount = count; busy = false;
                                failedPage = -1;
                                updateZoomLabel();
                                previous.setEnabled(requested > 0); next.setEnabled(requested + 1 < count);
                                image.requestFocus();
                            });
                        }
                    } catch (Exception error) {
                        if (result != null) result.recycle();
                        runOnUiThread(() -> {
                            if (!disposed && generation == ticket) {
                                busy = false; failedPage = requested;
                                label.setText(title + " · Sheet " + (requested + 1) + " failed · OK Retry / BACK return");
                                sizing.setText("Retry");
                                image.error(error.getMessage() == null ? error.getClass().getSimpleName() : error.getMessage());
                                image.requestFocus();
                            }
                        });
                    }
                });
            } catch (java.util.concurrent.RejectedExecutionException error) {
                busy = false; failedPage = requested; sizing.setText("Retry");
                label.setText(title + " · Renderer busy · OK Retry / BACK return");
                image.error("Renderer busy; retry this sheet"); image.requestFocus();
            }
        }

        void dispose() {
            disposed = true; generation++;
            if (pending != null) pending.cancel(false);
            pdfJobs.purge();
            image.clear();
            if (bitmap != null) { bitmap.recycle(); bitmap = null; }
        }
    }

    @Override public void onDestroy() {
        clearPanelCache();
        disposeExtraStreams();
        if(cachedLibraryScreen!=null)cachedLibraryScreen.dispose();
        if (background != null) background.dispose();
        for (View widget : widgets) if (widget instanceof PdfSurface) ((PdfSurface)widget).dispose();
        pdfJobs.shutdownNow();
        super.onDestroy();
    }
    private final Runnable refresh = new Runnable() {
        @Override public void run() {
            if (!resumed) return;
            if(libraryScreen==null)refreshWorkspace();
            refreshAudioControl();
            if(observerView!=null)observerView.refresh();
            updates.postDelayed(this, 250);
        }
    };

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        setVolumeControlStream(AudioManager.STREAM_MUSIC);
        getWindow().getDecorView().setSystemUiVisibility(
            View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
            | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY);
        frame = new FrameLayout(this);
        frame.setBackgroundColor(0xff000000);
        themeWallpaper = new ImageView(this);
        themeWallpaper.setScaleType(ImageView.ScaleType.CENTER_CROP);
        int wallpaper = getResources().getIdentifier("undergrowth", "drawable", getPackageName());
        if (wallpaper == 0) throw new IllegalStateException("Bundled Undergrowth wallpaper missing");
        themeWallpaper.setImageResource(wallpaper);
        themeWallpaper.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        frame.addView(themeWallpaper, new FrameLayout.LayoutParams(-1, -1));
        background = new GpuBackground();
        frame.addView(background, new FrameLayout.LayoutParams(-1, -1));
        video = new VideoView(this);
        video.setZOrderMediaOverlay(true); // Above shader surface, below native UI.
        FrameLayout.LayoutParams geometry = new FrameLayout.LayoutParams(-1, -1, Gravity.CENTER);
        frame.addView(video, geometry);
        status = new TextView(this);
        status.setTextColor(0xffffffff);
        status.setTextSize(24);
        status.setGravity(Gravity.CENTER);
        frame.addView(status, new FrameLayout.LayoutParams(-1, -1));
        muteButton = new Button(this);
        muteButton.setTextSize(16);
        muteButton.setOnClickListener(v -> setSurfaceMuted(!nativeIsMuted()));
        frame.addView(muteButton, new FrameLayout.LayoutParams(-2, -2, Gravity.TOP | Gravity.LEFT));
        setSurfaceMuted(getIntent().getBooleanExtra("muted", false));
        tunerEndpoint = getIntent().getStringExtra("tuner_endpoint");
        if (tunerEndpoint != null) getPreferences(0).edit().putString("tuner_endpoint", tunerEndpoint).apply();
        else tunerEndpoint = getPreferences(0).getString("tuner_endpoint", "");
        if(!TUNER_CONTROL_AVAILABLE) tunerEndpoint="";
        channelButton = new Button(this);
        channelButton.setText("Channels"); channelButton.setTextSize(14);
        channelButton.setOnClickListener(v -> showChannels());
        FrameLayout.LayoutParams channelChrome = new FrameLayout.LayoutParams(-2,-2,Gravity.TOP|Gravity.LEFT);
        channelChrome.topMargin = barHeight();
        frame.addView(channelButton, channelChrome);
        channelButton.setVisibility(View.GONE);
        setContentView(frame);
        libraryEndpoint = getIntent().getStringExtra("library_endpoint");
        if(libraryEndpoint != null) getPreferences(0).edit().putString("library_endpoint",libraryEndpoint).apply();
        else libraryEndpoint = getPreferences(0).getString("library_endpoint", "");
        consoleAppearance = getIntent().getStringExtra("console_appearance");
        if(consoleAppearance != null) getPreferences(0).edit().putString("console_appearance",consoleAppearance).apply();
        else consoleAppearance = getPreferences(0).getString("console_appearance", "");
        selector = new LinearLayout(this);
        selector.setOrientation(LinearLayout.HORIZONTAL);
        HorizontalScrollView scroller = new HorizontalScrollView(this);
        workspaceSelector = scroller;
        scroller.setHorizontalScrollBarEnabled(false);
        scroller.setFillViewport(true);
        scroller.setClipToPadding(false);
        scroller.setPadding(dp(ShellChrome.barPadding()), 0, dp(ShellChrome.barPadding()), 0);
        selector.setClipChildren(false);
        selector.setClipToPadding(false);
        scroller.addView(selector);
        FrameLayout.LayoutParams bar = new FrameLayout.LayoutParams(-1, barHeight(), Gravity.TOP);
        frame.addView(scroller, bar);
        workspaceHeader = new TextView(this);
        workspaceHeader.setSingleLine(true);
        workspaceHeader.setEllipsize(android.text.TextUtils.TruncateAt.END);
        workspaceHeader.setTextSize(13);
        workspaceHeader.setGravity(Gravity.CENTER_VERTICAL | Gravity.RIGHT);
        workspaceHeader.setPadding(dp(12), 0, dp(16), 0);
        workspaceHeader.setVisibility(View.GONE);
        frame.addView(workspaceHeader, new FrameLayout.LayoutParams(0, barHeight(), Gravity.TOP | Gravity.RIGHT));
        observerView=new ObserverView();
        FrameLayout.LayoutParams observerBounds=new FrameLayout.LayoutParams(dp(340),dp(62),Gravity.BOTTOM|Gravity.CENTER_HORIZONTAL);
        observerBounds.bottomMargin=dp(24);
        frame.addView(observerView,observerBounds);
        controlNoticeView=new ControlNoticeView();
        frame.addView(controlNoticeView,new FrameLayout.LayoutParams(-1,-1));
        String controlError = nativeStartControl(getFilesDir().getAbsolutePath());
        if (controlError == null || !controlError.isEmpty()) {
            showStatus("Control unavailable: " + controlError);
        }
        frame.addOnLayoutChangeListener((v, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
            if(pendingLibraryOpen) frame.post(() -> openRequestedLibrary());
            if (right - left != oldRight - oldLeft || bottom - top != oldBottom - oldTop) {
                applySurfaceGeometry();
            }
        });
        video.setOnPreparedListener(player -> {
            preparedPlayer = player;
            setSurfaceMuted(nativeIsMuted());
            status.setVisibility(View.GONE);
            if (resumed) video.start();
        });
        video.setOnErrorListener((player, what, extra) -> {
            showStatus("Playback failed (" + what + ", " + extra + ")");
            return true;
        });
        video.setOnCompletionListener(player -> showStatus("Stream ended"));
        applyIntent(getIntent());
    }

    private void showStatus(String text) {
        status.setText(text);
        status.setVisibility(View.VISIBLE);
    }

    private void showChannels() {
        if (tunerBusy || videoSurfaceId.isEmpty() || !tunerWorkspace()) return;
        tunerBusy = true; channelButton.setEnabled(false);
        final String surface = videoSurfaceId;
        final String workspace = currentWorkspaceId;
        new Thread(() -> {
            String raw = nativeTunerControl(tunerEndpoint, "", surface, getFilesDir().getAbsolutePath());
            updates.post(() -> {
                tunerBusy = false; channelButton.setEnabled(true);
                if (!tunerWorkspace() || !surface.equals(videoSurfaceId) || !workspace.equals(currentWorkspaceId)) return;
                try {
                    JSONObject reply = new JSONObject(raw);
                    if (reply.has("error")) throw new IllegalArgumentException(reply.getString("error"));
                    JSONArray channels = reply.getJSONArray("channels");
                    if (channels.length() > 1024) throw new IllegalArgumentException("Channel catalog too large");
                    if (!resumed || !surface.equals(videoSurfaceId) || !workspace.equals(currentWorkspaceId)) return;
                    renderChannels(channels, surface);
                } catch (Exception e) { android.util.Log.e("CanvasTuner", "Channel browser failed", e); showStatus("Channels unavailable: "+e.getMessage()); }
            });
        },"canvas-tuner-catalog").start();
    }

    private void closeChannels() {
        if (channelScreen != null) frame.removeView(channelScreen);
        channelScreen=null; channelScreenSurface=""; channelScreenWorkspace="";
        if (!videoSurfaceId.isEmpty()) video.setVisibility(View.VISIBLE);
        if (channelButton != null && channelButton.getVisibility()==View.VISIBLE) channelButton.requestFocus();
    }

    // This replaces the video tile in-place; it is not an Android dialog/window.
    private void renderChannels(JSONArray channels, String surface) throws Exception {
        closeChannels();
        JSONObject palette=new JSONObject(nativeProjection(frame.getWidth(),frame.getHeight())).getJSONObject("palette");
        int foreground=Color.parseColor(palette.getString("text")), muted=Color.parseColor(palette.getString("text_muted"));
        channelScreen=new LinearLayout(this); channelScreen.setOrientation(LinearLayout.VERTICAL);
        channelScreen.setBackgroundColor(Color.parseColor(palette.getString("background")));
        channelScreen.setPadding(dp(20),dp(14),dp(20),dp(14)); channelScreenSurface=surface; channelScreenWorkspace=currentWorkspaceId;
        LinearLayout heading=new LinearLayout(this); heading.setGravity(Gravity.CENTER_VERTICAL);
        TextView title=new TextView(this); title.setText("LIVE TV  /  CHANNELS"); title.setTextColor(foreground); title.setTextSize(20);
        heading.addView(title,new LinearLayout.LayoutParams(0,-2,1));
        Button back=new Button(this); back.setId(View.generateViewId()); back.setText("Back to TV"); back.setTextSize(12); styleControl(back,palette,false);
        back.setOnClickListener(v -> closeChannels()); heading.addView(back,new LinearLayout.LayoutParams(-2,dp(44)));
        channelScreen.addView(heading,new LinearLayout.LayoutParams(-1,dp(50)));
        TextView preview=new TextView(this); preview.setText(surfaceTitle); preview.setTextColor(foreground); preview.setTextSize(30);
        preview.setPadding(0,dp(8),0,dp(4)); channelScreen.addView(preview,new LinearLayout.LayoutParams(-1,-2));
        TextView instructions=new TextView(this); instructions.setText(channels.length()+" channels  ·  ↑ ↓ browse  ·  Select to watch  ·  Back returns to TV");
        instructions.setTextColor(muted); instructions.setTextSize(12); instructions.setPadding(0,0,0,dp(14));
        channelScreen.addView(instructions,new LinearLayout.LayoutParams(-1,-2));
        ScrollView scroll=new ScrollView(this); LinearLayout rows=new LinearLayout(this); rows.setOrientation(LinearLayout.VERTICAL);
        scroll.addView(rows); channelScreen.addView(scroll,new LinearLayout.LayoutParams(-1,0,1));
        Button initial=null, previousRow=null;
        for(int i=0;i<channels.length();i++) {
            JSONObject item=channels.getJSONObject(i); final String number=item.getString("channel"), name=item.getString("name");
            boolean selected=surfaceTitle.startsWith(number+" · ");
            Button row=new Button(this); row.setId(View.generateViewId()); row.setFocusableInTouchMode(true);
            row.setNextFocusRightId(back.getId());
            if(previousRow!=null) { previousRow.setNextFocusDownId(row.getId()); row.setNextFocusUpId(previousRow.getId()); }
            else row.setNextFocusUpId(back.getId());
            previousRow=row;
            row.setText(number+"     "+name+"    "+(selected?"● Playing":"›"));
            row.setGravity(Gravity.CENTER_VERTICAL|Gravity.LEFT); row.setTextSize(18); row.setPadding(dp(16),0,dp(16),0);
            row.setAllCaps(false); styleControl(row,palette,selected);
            row.setContentDescription("Watch "+name+", channel "+number);
            row.setOnFocusChangeListener((v,focused) -> { if(focused) preview.setText(number+"  ·  "+name); });
            row.setOnClickListener(v -> { closeChannels(); tuneChannel(number,surface); });
            LinearLayout.LayoutParams bounds=new LinearLayout.LayoutParams(-1,dp(54)); bounds.bottomMargin=dp(4); rows.addView(row,bounds);
            if(initial==null || selected) initial=row;
        }
        FrameLayout.LayoutParams bounds=new FrameLayout.LayoutParams((FrameLayout.LayoutParams)video.getLayoutParams());
        video.setVisibility(View.INVISIBLE); status.setVisibility(View.GONE);
        frame.addView(channelScreen,bounds); channelScreen.bringToFront();
        if(initial!=null) { back.setNextFocusDownId(initial.getId()); back.setNextFocusLeftId(initial.getId()); initial.requestFocus(); }
        else back.requestFocus();
    }

    @Override public void onBackPressed() {
        if(libraryScreen!=null) { if(!libraryScreen.goBack())closeLibrary(); return; }
        if(channelScreen!=null) { closeChannels(); return; }
        super.onBackPressed();
    }

    private void tuneChannel(String channel, String surface) {
        if (tunerBusy || !tunerWorkspace() || !surface.equals(videoSurfaceId)) return;
        tunerBusy=true; channelButton.setEnabled(false); showStatus("Tuning "+channel+"…");
        new Thread(() -> {
            String raw=nativeTunerControl(tunerEndpoint,channel,surface,getFilesDir().getAbsolutePath());
            updates.post(() -> {
                tunerBusy=false; channelButton.setEnabled(true);
                if (!tunerWorkspace() || !surface.equals(videoSurfaceId)) return;
                try {
                    JSONObject reply=new JSONObject(raw);
                    if (reply.has("error")) throw new IllegalArgumentException(reply.getString("error"));
                    lastProjection=""; refreshWorkspace();
                } catch(Exception e) { showStatus("Tuning failed: "+e.getMessage()); }
            });
        },"canvas-tuner-select").start();
    }

    private void applyIntent(Intent intent) {
        if(intent.getBooleanExtra("open_library",false)) {
            pendingLibraryOpen=true;
            frame.post(() -> openRequestedLibrary());
            return;
        }
        if (workspaceMode) return;
        applySurfaceGeometry();
        preparedPlayer = null;
        video.stopPlayback();
        if (intent.hasExtra("muted")) setSurfaceMuted(intent.getBooleanExtra("muted", false));
        String url = intent.getStringExtra("stream_url");
        if (url == null) { showStatus("fungOS Fire TV — select a stream"); return; }
        String error = nativeValidate(url);
        if (error == null || !error.isEmpty()) {
            showStatus(error == null ? "Command validation failed" : error);
            return;
        }
        showStatus("Connecting…");
        currentUrl = url;
        video.setVideoURI(Uri.parse(url));
    }

    private void setSurfaceMuted(boolean muted) {
        float gain = nativeSetMuted(muted);
        if (preparedPlayer != null) preparedPlayer.setVolume(gain, gain);
        ShellChrome.audioState(muteButton, surfaceTitle, muted);
    }

    private void refreshAudioControl() {
        try {
            String id = videoSurfaceId == null || videoSurfaceId.isEmpty() ? "standalone-video" : videoSurfaceId;
            JSONObject snapshot = new JSONObject(nativeAudioUpdate(id, surfaceTitle, currentUrl,
                preparedPlayer != null && resumed && video.getVisibility() == View.VISIBLE, nativeIsMuted()));
            if (snapshot.has("error")) throw new IllegalStateException(snapshot.getString("error"));
            if (controlNoticeView != null) controlNoticeView.update(snapshot);
            if (snapshot.isNull("pending")) return;
            JSONObject pending = snapshot.getJSONObject("pending");
            boolean muted = pending.getBoolean("muted");
            // The UI thread owns this exact player. Report applied only after
            // MediaPlayer accepts source-local gain; never touch master volume.
            float gain = muted ? 0.0f : 1.0f;
            preparedPlayer.setVolume(gain, gain);
            if (!nativeAudioAcknowledge(pending.getString("request_id"), pending.getLong("revision"), muted))
                throw new IllegalStateException("Audio acknowledgement rejected");
            setSurfaceMuted(muted);
        } catch (Exception error) {
            showStatus("Audio control failed: " + error.getMessage());
        }
    }

    // Rust's shared Canvas layout solver owns geometry. Android only applies
    // pixel bounds and preserves decoded video aspect ratio within them.
    private void applySurfaceGeometry() {
        if (workspaceMode) { lastProjection = ""; return; }
        boolean windowed = getIntent().getBooleanExtra("windowed", false);
        int width = frame.getWidth();
        int height = frame.getHeight();
        if (width <= 0 || height <= 0) return;
        int[] bounds = nativeVideoFrame(width, height, windowed);
        if (bounds == null || bounds.length != 4) {
            showStatus("Surface layout failed");
            return;
        }
        FrameLayout.LayoutParams geometry = new FrameLayout.LayoutParams(bounds[2], bounds[3], Gravity.TOP | Gravity.LEFT);
        geometry.leftMargin = bounds[0];
        geometry.topMargin = bounds[1];
        video.setLayoutParams(geometry);
        FrameLayout.LayoutParams chrome = new FrameLayout.LayoutParams(dp(ShellChrome.AUDIO_WIDTH_DP), dp(ShellChrome.AUDIO_HEIGHT_DP), Gravity.TOP | Gravity.LEFT);
        chrome.leftMargin = width * 4 / 100;
        chrome.topMargin = height * 4 / 100;
        muteButton.setLayoutParams(chrome);
    }

    @Override protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        applyIntent(intent);
    }

    /** Paints bounded gfx signal records; owns no MIDI, FFT or waveform reduction. */
    private final class SignalView extends LinearLayout {
        private final TextView footer;
        private final String surfaceId;
        private final boolean taskDeck;
        private final boolean geoMap;
        private View taskPlot;
        boolean navigate(String action) {
            try {
                if (geoMap && !action.equals("focus")) nativeMapManual(surfaceId);
                JSONObject result = new JSONObject(nativeTaskNavigate(surfaceId, action));
                if (result.has("error")) throw new IllegalStateException(result.getString("error"));
                if (taskPlot != null) taskPlot.invalidate();
                return result.optBoolean("handled", false);
            } catch (Exception error) { footer.setText("Navigation unavailable: " + error.getMessage()); return false; }
        }
        SignalView(String id, JSONObject spec, JSONObject palette) throws Exception {
            super(TvActivity.this);
            surfaceId=id;
            geoMap=spec.optJSONObject("frame") != null && spec.getJSONObject("frame").optString("view").equals("geo_map");
            taskDeck=spec.optJSONObject("frame") != null && (spec.getJSONObject("frame").optString("view").equals("task_deck") || spec.getJSONObject("frame").optString("view").equals("geo_map"));
            setOrientation(VERTICAL);
            setBackgroundColor(Color.parseColor(palette.getString("surface")));
            int pad = Math.round(16 * getResources().getDisplayMetrics().density);
            setPadding(pad, pad, pad, pad);
            TextView title = new TextView(TvActivity.this);
            title.setText(spec.getString("title")); title.setTextSize(20);
            title.setTextColor(Color.parseColor(palette.getString("text")));
            final boolean frameless = spec.optString("chrome", "auto").equals("frameless");
            if (!frameless && !spec.getString("title").isEmpty())
                addView(title, new LinearLayout.LayoutParams(-1, -2));
            footer = new TextView(TvActivity.this); footer.setTextSize(13);
            footer.setTextColor(Color.parseColor(palette.getString("text_muted")));
            // Provider cadence governs freshness, not how quickly an accepted
            // frame or local selection becomes visible. Geometry remains cached.
            final long interval = spec.optJSONObject("frame") != null
                && spec.getJSONObject("frame").optString("view").equals("geo_map")
                ? 33L : Math.max(33L, Math.min(250L, spec.optLong("refresh_ms", 250L)));
            final String appearance = !consoleAppearance.isEmpty() ? consoleAppearance : palette.optJSONObject("console_appearance") != null ? palette.getJSONObject("console_appearance").toString() : "{}";
            final int[] inks = new int[]{
                Color.parseColor(palette.getString("background")), Color.parseColor(palette.getString("surface")),
                Color.parseColor(palette.getString("surface_muted")), Color.parseColor(palette.getString("background")),
                Color.parseColor(palette.getString("text")), Color.parseColor(palette.getString("text_muted")),
                Color.parseColor(palette.getString("primary")), Color.parseColor(palette.getString("info")),
                Color.parseColor(palette.getString("success")), Color.parseColor(palette.getString("warning")),
                Color.parseColor(palette.getString("danger")), Color.parseColor(palette.getString("success")),
                Color.parseColor(palette.getString("warning")), Color.parseColor(palette.getString("border")),
                Color.BLACK, Color.parseColor(palette.getString("text"))};
            View plot = new View(TvActivity.this) {
                final Paint ink = new Paint(Paint.ANTI_ALIAS_FLAG);
                final android.graphics.Path lineBatch = new android.graphics.Path();
                String previous = "";
                float[] records;
                JSONArray labels;
                int oldWidth, oldHeight;
                final java.util.HashMap<String, Float> needleLevels = new java.util.HashMap<>();
                long lastNeedleFrame;
                String lastMapObservation = "";
                long lastKioskPoll;
                final MapRasterLayer raster = new MapRasterLayer();
                final MapFramePump mapPump = geoMap ? new MapFramePump(id,appearance) : null;
                @Override protected void onDraw(android.graphics.Canvas target) {
                    super.onDraw(target);
                    if (!resumed || libraryScreen!=null || getWidth() < 2 || getHeight() < 2) return;
                    try {
                        // GFX geometry is logical pixels, like the shared analog clock.
                        // Shell text/padding already use density-scaled Android units.
                        final float density = getResources().getDisplayMetrics().density;
                        if (Float.isNaN(density) || Float.isInfinite(density) || density <= 0f)
                            throw new IllegalStateException("Invalid signal display density");
                        final int logicalWidth = Math.max(2, Math.round(getWidth() / density));
                        final int logicalHeight = Math.max(2, Math.round(getHeight() / density));
                        long paintTime = android.os.SystemClock.elapsedRealtime();
                        MapFramePump.Ready mapReady = geoMap ? mapPump.request(logicalWidth,logicalHeight) : null;
                        if(geoMap && mapReady==null) {postInvalidateDelayed(interval);return;}
                        JSONObject snapshot = geoMap ? mapReady.snapshot : new JSONObject(nativeSignalSnapshot(id));
                        if (snapshot.has("error")) throw new IllegalStateException(snapshot.getString("error"));
                        JSONObject data = snapshot.getJSONObject("data");
                        String state = data.getString("state");
                        String label = state + " · " + data.optString("detail", "");
                        if (!footer.getText().toString().equals(label)) footer.setText(label);
                        JSONObject frame = snapshot.getJSONObject("frame");
                        if (geoMap) raster.request(frame.getJSONObject("map"),logicalWidth,logicalHeight);
                        String viewKind = frame.optString("view");
                        if (viewKind.equals("geo_map") && !data.optString("observed_at").equals(lastMapObservation)) {
                            lastMapObservation = data.optString("observed_at");
                            android.util.Log.i("CanvasMap", "observation=" + lastMapObservation
                                + " motion=" + snapshot.optJSONObject("motion")
                                + " paths=" + frame.getJSONObject("map").getJSONArray("paths").length());
                        }
                        if ((state.equals("stale") || state.equals("no_data"))
                            && (viewKind.equals("attention") || viewKind.equals("task_deck")))
                            frame.getJSONObject("panel").put("fresh", false);
                        // Static briefs have their own source footer, not a live-sensor heartbeat.
                        boolean staticBrief = frame.optString("view").equals("brief");
                        footer.setVisibility(frameless && (state.equals("ready")
                            || (staticBrief && state.equals("no_data"))) ? View.GONE : View.VISIBLE);
                        String raw = geoMap ? mapReady.raw : frame.toString();
                        String paintRaw = raw;
                        int[] frameInks = inks;
                        boolean needleAnimation = false;
                        if (frame.optString("view").equals("console")) {
                            JSONObject meter = frame.getJSONObject("panel").getJSONObject("meter");
                            if (meter.optString("face").equals("highlight")) {
                                frameInks = inks.clone();
                                frameInks[15] = Color.parseColor(palette.optString("meter_face", "#eadfbd"));
                                frameInks[14] = Color.parseColor(palette.optString("meter_ink", "#302a21"));
                            }
                            int smoothing = meter.optInt("smoothing_ms", 0);
                            if (smoothing > 0 && meter.optString("style").equals("needle") && state.equals("ready")) {
                                long now = android.os.SystemClock.elapsedRealtime();
                                float blend = lastNeedleFrame == 0 ? 1f : (float)(1 - Math.exp(-Math.min(250, now-lastNeedleFrame)/(double)smoothing));
                                lastNeedleFrame = now;
                                JSONObject animated = new JSONObject(raw);
                                JSONArray strips = animated.getJSONObject("panel").getJSONArray("strips");
                                for (int n=0; n<strips.length(); n++) {
                                    JSONObject strip = strips.getJSONObject(n);
                                    String key = strip.getString("name");
                                    if (strip.isNull("peak_dbfs")) { needleLevels.remove(key); continue; }
                                    float goal = (float)strip.getDouble("peak_dbfs");
                                    float old = needleLevels.containsKey(key) ? needleLevels.get(key) : goal;
                                    float shown = old + (goal-old)*blend;
                                    needleLevels.put(key, shown); strip.put("peak_dbfs", shown);
                                }
                                paintRaw = animated.toString(); needleAnimation = true;
                            }
                        }
                        if (needleAnimation || !raw.equals(previous) || oldWidth != logicalWidth || oldHeight != logicalHeight) {
                            records = geoMap ? mapReady.records : nativeSignalFrame(logicalWidth, logicalHeight, paintRaw, appearance);
                            if (!raw.equals(previous) || oldWidth != logicalWidth || oldHeight != logicalHeight)
                            labels = geoMap ? mapReady.labels : new JSONArray(nativeSignalLabels(logicalWidth, logicalHeight, raw));
                            if (records == null || records.length % 8 != 0) throw new IllegalStateException("Invalid signal paint records");
                            previous = raw; oldWidth = logicalWidth; oldHeight = logicalHeight;
                        }
                        final int paintSave = target.save();
                        try {
                            target.scale(density, density);
                            // Freshness is explicit in the footer and frame, never whole-panel opacity.
                            ink.setAlpha(255);
                            for (int i = 0; i < records.length; i += 8) {
                                ink.setColor(frameInks[(int)records[i+6]]);
                                ink.setAlpha(255);
                                if (records[i] == 0f) {
                                    ink.setStyle(Paint.Style.FILL);
                                    target.drawRoundRect(records[i+1], records[i+2], records[i+1]+records[i+3],
                                        records[i+2]+records[i+4], records[i+5], records[i+5], ink);
                                } else if (records[i] == 5f) {
                                    ink.setStyle(Paint.Style.STROKE); ink.setStrokeCap(Paint.Cap.ROUND);
                                    ink.setStrokeWidth(records[i+5]);
                                    // Preserve ordering while batching consecutive equal-style
                                    // independent segments into one retained GPU path command.
                                    lineBatch.reset();
                                    int end=i;
                                    while(true) {
                                        lineBatch.moveTo(records[end+1],records[end+2]);
                                        lineBatch.lineTo(records[end+3],records[end+4]);
                                        int next=end+8;
                                        if(next>=records.length || records[next]!=5f
                                            || records[next+5]!=records[i+5]
                                            || records[next+6]!=records[i+6])break;
                                        end=next;
                                    }
                                    target.drawPath(lineBatch,ink); i=end;
                                } else throw new IllegalStateException("Unsupported signal primitive");
                                if (geoMap && i == 0) raster.draw(target);
                            }
                            if (geoMap) raster.attribution(target);
                            ink.setStyle(Paint.Style.FILL);
                            // The shared CodeGrid column geometry owns alignment; the
                            // host supplies glyphs without turning it into a new layout.
                            ink.setTypeface(viewKind.equals("code_grid")
                                ? android.graphics.Typeface.MONOSPACE : android.graphics.Typeface.DEFAULT);
                            for (int i = 0; i < labels.length(); i++) {
                                JSONObject text = labels.getJSONObject(i), box = text.getJSONObject("bounds");
                                float x = (float)box.getDouble("x"), y = (float)box.getDouble("y");
                                float width = (float)box.getDouble("width"), height = (float)box.getDouble("height");
                                // Shared label geometry supplies hierarchy; allow headings beyond body size.
                                ink.setTextSize(Math.max(10f, Math.min(48f, height * 0.8f)));
                                ink.setColor(frameInks[text.getInt("role")]); ink.setAlpha(255);
                                String value = text.getString("text");
                                if (ink.measureText(value) > width) {
                                    int count = ink.breakText(value, true, Math.max(0f, width-ink.measureText("…")), null);
                                    value = value.substring(0, count) + "…";
                                }
                                target.save(); target.clipRect(x, y, x+width, y+height);
                                target.drawText(value, x, y-ink.ascent(), ink); target.restore();
                            }
                        } finally {
                            // Unwind nested label clipping too, before the error UI.
                            target.restoreToCount(paintSave);
                        }
                    } catch (Exception error) {
                        footer.setVisibility(View.VISIBLE);
                        footer.setText("Signal unavailable: " + error.getMessage());
                    }
                    postInvalidateDelayed(interval);
                }
            };
            addView(plot, new LinearLayout.LayoutParams(-1, 0, 1f));
            addView(footer, new LinearLayout.LayoutParams(-1, -2));
            taskPlot=plot;
            if (taskDeck) {
                setFocusable(true); setFocusableInTouchMode(true);
                setContentDescription("Interactive surface. Direction keys select. OK opens details. Back returns.");
                setOnFocusChangeListener((view, focused) -> { if (focused) navigate("focus"); });
                setOnKeyListener((view, key, event) -> {
                    if (event.getAction()!=KeyEvent.ACTION_DOWN) return false;
                    String action=key==KeyEvent.KEYCODE_DPAD_LEFT ? "left" : key==KeyEvent.KEYCODE_DPAD_RIGHT ? "right"
                        : key==KeyEvent.KEYCODE_DPAD_UP ? "up" : key==KeyEvent.KEYCODE_DPAD_DOWN ? "down"
                        : key==KeyEvent.KEYCODE_DPAD_CENTER || key==KeyEvent.KEYCODE_ENTER ? "open"
                        : key==KeyEvent.KEYCODE_BACK ? "back" : "";
                    return !action.isEmpty() && navigate(action);
                });
            }
        }
    }

    /** Generic themed primitive; providers and state interpretation stay in Lua. */
    private final class StatusListView extends LinearLayout {
        private final TextView heading, health;
        private final LinearLayout rows;
        private JSONObject data;
        private long observedAt;
        private long staleAfter = 30000;
        private final int normal, warning;
        private final Runnable freshness = new Runnable() {
            @Override public void run() {
                if (!resumed || !isAttachedToWindow()) return;
                updateHealth(); updates.postDelayed(this, 1000);
            }
        };
        StatusListView(JSONObject spec, JSONObject palette) throws Exception {
            super(TvActivity.this);
            setOrientation(VERTICAL); setPadding(dp(18), dp(14), dp(18), dp(14));
            normal = Color.parseColor(palette.getString("text_muted"));
            warning = Color.parseColor(palette.getString("warning"));
            GradientDrawable shape = new GradientDrawable();
            shape.setColor(Color.parseColor(palette.getString("surface")));
            shape.setCornerRadius(dp(12));
            shape.setStroke(dp(1), Color.parseColor(palette.getString("border")));
            setBackground(shape);
            heading = new TextView(TvActivity.this); heading.setTextSize(20);
            heading.setTextColor(Color.parseColor(palette.getString("text")));
            addView(heading, new LinearLayout.LayoutParams(-1, -2));
            rows = new LinearLayout(TvActivity.this); rows.setOrientation(VERTICAL);
            addView(rows, new LinearLayout.LayoutParams(-1, 0, 1));
            health = new TextView(TvActivity.this); health.setTextSize(12);
            addView(health, new LinearLayout.LayoutParams(-1, -2));
            update(spec, palette);
        }
        void update(JSONObject spec, JSONObject palette) throws Exception {
            heading.setText(spec.optString("title", "Status"));
            data = spec.optJSONObject("data"); observedAt = 0;
            staleAfter = Math.max(30000, Math.min(300000, spec.optLong("refresh_ms", 10000) * 3));
            if (data != null && !data.isNull("observed_at")) {
                java.text.SimpleDateFormat parser = new java.text.SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'", java.util.Locale.ROOT);
                parser.setTimeZone(java.util.TimeZone.getTimeZone("UTC")); parser.setLenient(false);
                try { observedAt = parser.parse(data.getString("observed_at")).getTime(); }
                catch (java.text.ParseException error) { android.util.Log.w("CanvasStatus", "Invalid observation timestamp"); }
            }
            rows.removeAllViews();
            if (spec.optString("kind").equals("time_series")) {
                rows.addView(new TrendView(spec, palette), new LinearLayout.LayoutParams(-1, -1));
                updateHealth(); return;
            }
            JSONArray nodes = spec.getJSONArray("nodes");
            for (int i = 0; i < nodes.length(); i++) {
                JSONObject node = nodes.getJSONObject(i);
                LinearLayout row = new LinearLayout(TvActivity.this); row.setGravity(Gravity.CENTER_VERTICAL);
                TextView label = new TextView(TvActivity.this); label.setTextSize(17);
                label.setTextColor(Color.parseColor(palette.getString("text_muted")));
                label.setText(node.getString("label"));
                row.addView(label, new LinearLayout.LayoutParams(0, -2, 0.38f));
                TextView value = new TextView(TvActivity.this);
                String level = node.optString("state", "neutral");
                value.setTextColor(Color.parseColor(palette.getString(level.equals("good") ? "success"
                    : level.equals("warning") ? "warning" : level.equals("critical") ? "danger" : "text")));
                String detail = node.isNull("detail") ? "—" : node.optString("detail", "—");
                value.setTextSize(detail.length() > 20 ? 16 : 24);
                value.setText(detail); value.setMaxLines(3);
                value.setEllipsize(android.text.TextUtils.TruncateAt.END);
                row.addView(value, new LinearLayout.LayoutParams(0, -2, 0.62f));
                rows.addView(row, new LinearLayout.LayoutParams(-1, 0, 1));
            }
            updateHealth();
        }
        private void updateHealth() {
            String state = data == null ? "loading" : data.optString("state", "loading");
            long age = System.currentTimeMillis() - observedAt;
            boolean stale = observedAt <= 0 || age < -5000 || age > staleAfter;
            String label = state.equals("ready") || state.equals("partial")
                ? (stale ? "Stale" : "Live") : state;
            String detail = data == null ? "" : data.optString("detail", "");
            health.setText(label + (detail.isEmpty() ? "" : " · " + detail));
            health.setTextColor(label.equals("Live") ? normal : warning);
            rows.setAlpha(label.equals("Live") ? 1f : 0.5f);
        }
        @Override protected void onAttachedToWindow() { super.onAttachedToWindow(); updates.post(freshness); }
        @Override protected void onDetachedFromWindow() { updates.removeCallbacks(freshness); super.onDetachedFromWindow(); }
    }

    /** Retained generic time-series primitive, using the shared spec and palette. */
    private final class TrendView extends View {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final JSONArray series;
        private final String unit;
        private final int foreground, muted, primary, border;
        private double minX = Double.POSITIVE_INFINITY, maxX = Double.NEGATIVE_INFINITY;
        private double minY = 0, maxY = 1, latestX = Double.NEGATIVE_INFINITY, latestY;
        TrendView(JSONObject spec, JSONObject palette) throws Exception {
            super(TvActivity.this); series = spec.getJSONArray("series"); unit = spec.optString("unit", "");
            foreground = Color.parseColor(palette.getString("text")); muted = Color.parseColor(palette.getString("text_muted"));
            primary = Color.parseColor(palette.getString("primary")); border = Color.parseColor(palette.getString("border"));
            for (int s = 0; s < series.length(); s++) {
                JSONArray points = series.getJSONObject(s).getJSONArray("points");
                for (int p = 0; p < points.length(); p++) {
                    JSONObject point = points.getJSONObject(p); double x = point.getDouble("x"), y = point.getDouble("y");
                    minX = Math.min(minX, x); maxX = Math.max(maxX, x); minY = Math.min(minY, y); maxY = Math.max(maxY, y);
                    if (x >= latestX) { latestX = x; latestY = y; }
                }
            }
            if (maxX == minX) { minX -= 1; maxX += 1; }
            maxY += (maxY - minY) * 0.08;
        }
        @Override protected void onDraw(android.graphics.Canvas canvas) {
            super.onDraw(canvas);
            if (!Double.isFinite(minX) || getWidth() <= 0 || getHeight() <= 0) return;
            float left = dp(20), right = getWidth() - dp(12), top = dp(48), bottom = getHeight() - dp(16);
            if (bottom <= top || right <= left) return;
            paint.setColor(foreground); paint.setStyle(Paint.Style.FILL); paint.setTextSize(dp(26));
            canvas.drawText(String.format(java.util.Locale.ROOT, "%.0f %s", latestY, unit), 0, dp(32), paint);
            paint.setColor(border); paint.setStrokeWidth(dp(1));
            for (int i = 0; i < 3; i++) {
                float y = top + (bottom - top) * i / 2f;
                canvas.drawLine(left, y, right, y, paint);
            }
            paint.setColor(muted); paint.setTextSize(dp(10));
            canvas.drawText("0", 0, bottom, paint);
            paint.setColor(primary); paint.setStrokeWidth(dp(2));
            for (int s = 0; s < series.length(); s++) {
                JSONArray points = series.optJSONObject(s).optJSONArray("points");
                android.graphics.Path path = new android.graphics.Path();
                for (int p = 0; p < points.length(); p++) {
                    JSONObject point = points.optJSONObject(p);
                    float x = left + (float)((point.optDouble("x") - minX) / (maxX - minX)) * (right - left);
                    float y = bottom - (float)((point.optDouble("y") - minY) / (maxY - minY)) * (bottom - top);
                    if (p == 0) path.moveTo(x, y); else path.lineTo(x, y);
                    paint.setStyle(Paint.Style.FILL); canvas.drawCircle(x, y, dp(3), paint);
                }
                paint.setStyle(Paint.Style.STROKE); canvas.drawPath(path, paint);
            }
            paint.setStyle(Paint.Style.FILL);
        }
    }

    /** Dynamic primitive values are not geometry: don't re-rasterize PDFs per tick. */
    private void stripStatusValues(JSONObject surface) throws Exception {
        JSONObject content = surface.getJSONObject("content");
        if (!content.optString("kind").equals("widget")) return;
        JSONObject spec = content.getJSONObject("spec");
        if (spec.optString("kind").equals("rich_card")) {
            spec.remove("title"); spec.remove("body"); spec.remove("data"); spec.remove("citations");
            return;
        }
        if (spec.optString("kind").equals("signal_view")) {
            spec.remove("data");
            // A frame is live content; its variant is structural.
            JSONObject signal = spec.optJSONObject("frame");
            if (signal != null) spec.put("frame", new JSONObject().put("view", signal.optString("view")));
            return;
        }
        if (!spec.optString("kind").equals("status_list") && !spec.optString("kind").equals("time_series")) return;
        spec.remove("nodes"); spec.remove("series"); spec.remove("unit"); spec.remove("data"); spec.remove("title");
    }
    private String structuralProjection(JSONObject projection) throws Exception {
        JSONObject copy = new JSONObject(projection.toString()); copy.remove("revision");
        copy.remove("header");
        copy.remove("observer"); // Ephemeral state must never reconstruct videos/PDFs.
        JSONObject document = copy.getJSONObject("document"); document.remove("revision");
        JSONArray surfaces = document.getJSONArray("surfaces");
        for (int i = 0; i < surfaces.length(); i++) stripStatusValues(surfaces.getJSONObject(i));
        JSONArray tiles = copy.getJSONArray("tiles");
        for (int i = 0; i < tiles.length(); i++) stripStatusValues(tiles.getJSONObject(i).getJSONObject("surface"));
        return copy.toString();
    }

    @Override protected void onResume() {
        super.onResume();
        for (View widget : widgets) if (widget instanceof WorkspaceDemoView) ((WorkspaceDemoView)widget).onResume();
        resumed = true;
        for (View widget : widgets) if(widget instanceof ConwaysView) ((ConwaysView)widget).setActive(true);
        background.setActive(true);
        lastProjection = "";
        lastStructure = "";
        if (workspaceMode) currentUrl = "";
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        video.start();
        updates.post(refresh);
    }

    @Override protected void onPause() {
        for (View widget : widgets) if (widget instanceof WorkspaceDemoView) ((WorkspaceDemoView)widget).onPause();
        resumed = false;
        clearPanelCache();
        disposeExtraStreams();
        for (View widget : widgets) if(widget instanceof ConwaysView) ((ConwaysView)widget).setActive(false);
        refreshAudioControl(); // Revoke pending work before releasing the player.
        background.setActive(false);
        updates.removeCallbacks(refresh);
        preparedPlayer = null;
        video.stopPlayback(); // Release tuner when the app is no longer visible.
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        super.onPause();
    }

    @Override public boolean onKeyDown(int key, KeyEvent event) {
        for (View widget : widgets) {
            if (widget instanceof PdfSurface && widget.hasFocus() && ((PdfSurface)widget).remote(key,event)) return true;
            if (widget instanceof ConwaysView && (key == KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE || key == KeyEvent.KEYCODE_MEDIA_FAST_FORWARD || key == KeyEvent.KEYCODE_MEDIA_REWIND)) {
                if(event.getRepeatCount()==0) ((ConwaysView)widget).control(key == KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE ? "play-pause" : key == KeyEvent.KEYCODE_MEDIA_FAST_FORWARD ? "random" : "clear");
                return true;
            }
        }
        if (key == KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE || key == KeyEvent.KEYCODE_INFO || key == KeyEvent.KEYCODE_I) {
            for (View widget : widgets) {
                if (widget instanceof BoidsView && !((BoidsView)widget).footnote.isEmpty()) {
                    if(event.getRepeatCount()==0) ((BoidsView)widget).toggleFootnote();
                    return true;
                }
            }
        }
        if (key == KeyEvent.KEYCODE_MENU && selector.getChildCount() > 0) {
            for(int i=0;i<selector.getChildCount();i++) if(selector.getChildAt(i).isFocusable()) {
                selector.getChildAt(i).requestFocus(); break;
            }
            return true;
        }
        if (key == KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE) {
            if (video.isPlaying()) video.pause(); else video.start();
            return true;
        }
        return super.onKeyDown(key, event);
    }


    /** Presentation-only grouping; project identity and activation stay in Rust. */
    private JSONObject shellMenu;
    private String menuSection = "", menuProject = "";
    private boolean menuExpanded = false;
    private JSONObject menuDescriptor() throws Exception {
        if (shellMenu != null) return shellMenu;
        try (java.io.InputStream input = getAssets().open("canvas-menu.json")) {
            java.io.ByteArrayOutputStream bytes = new java.io.ByteArrayOutputStream();
            byte[] buffer = new byte[4096]; int count;
            while ((count=input.read(buffer)) != -1) {
                if (bytes.size()+count>65536) throw new java.io.IOException("Menu exceeds 64 KiB");
                bytes.write(buffer,0,count);
            }
            JSONObject menu = new JSONObject(new String(bytes.toByteArray(),java.nio.charset.StandardCharsets.UTF_8));
            if(menu.getInt("schema_version") != 1 || menu.getJSONArray("items").length()>32)
                throw new IllegalArgumentException("Unsupported menu descriptor");
            shellMenu=menu; return menu;
        } catch(java.io.FileNotFoundException absent) {
            shellMenu=new JSONObject(); shellMenu.put("items",new JSONArray()); return shellMenu;
        }
    }
    private boolean menuContains(JSONObject item, String selected) throws Exception {
        if(item.optString("project_id").equals(selected)) return true;
        JSONArray children=item.optJSONArray("children");
        if(children != null) for(int i=0;i<children.length();i++)
            if(children.getJSONObject(i).optString("project_id").equals(selected)) return true;
        return false;
    }
    private void addMenuChoice(JSONObject item, JSONObject projection, String focusedTag) throws Exception {
        JSONObject catalog=projection.getJSONObject("catalog"), palette=projection.getJSONObject("palette");
        String selected=catalog.getString("selected_project");
        JSONArray children=item.optJSONArray("children");
        final String project=item.optString("project_id"), section=item.optString("id");
        if(children != null && (section.isEmpty() || children.length()>32))
            throw new IllegalArgumentException("Invalid menu section");
        if(children == null && project.isEmpty()) throw new IllegalArgumentException("Menu choice needs a project");
        Button button=new Button(this); String tag=children == null ? project : "section:"+section;
        button.setTag(tag); button.setText(item.getString("label")+(children == null ? "" : section.equals(menuSection) ? " ▴" : " ▾"));
        button.setTextSize(ShellChrome.textSize()); ShellChrome.control(button,palette,menuContains(item,selected),true);
        button.setContentDescription(children == null ? "Open "+item.getString("label") : "Browse "+item.getString("label"));
        button.setOnClickListener(v -> {
            if(children != null) {
                menuSection=menuSection.equals(section) ? "" : section;
                try { renderWorkspaceMenu(projection); }
                catch(Exception error) { showStatus("Menu failed: "+error.getMessage()); }
            } else {
                String error=nativeSelectWorkspace(project);
                if(error == null || !error.isEmpty()) showStatus("Switch failed: "+error);
                else { lastProjection=""; refreshWorkspace(); }
            }
        });
        button.setMinHeight(0); button.setMinimumHeight(0);
        LinearLayout.LayoutParams bounds=new LinearLayout.LayoutParams(-2,dp(ShellChrome.controlHeight()));
        bounds.setMargins(dp(4),0,0,0); selector.addView(button,bounds);
        if(tag.equals(focusedTag)) button.requestFocus();
    }
    private void renderWorkspaceMenu(JSONObject projection) throws Exception {
        JSONObject catalog=projection.getJSONObject("catalog"), palette=projection.getJSONObject("palette");
        JSONObject menu=menuDescriptor(); JSONArray configured=menu.getJSONArray("items"), items=new JSONArray();
        java.util.HashSet<String> listed=new java.util.HashSet<>();
        for(int i=0;i<configured.length();i++) {
            JSONObject item=configured.getJSONObject(i);items.put(item);
            JSONArray children=item.optJSONArray("children");
            if(children==null) listed.add(item.getString("project_id"));
            else for(int j=0;j<children.length();j++) listed.add(children.getJSONObject(j).getString("project_id"));
        }
        JSONArray hidden=menu.optJSONArray("hidden_projects");
        if(hidden!=null) for(int i=0;i<hidden.length();i++) listed.add(hidden.getString(i));
        if(menu.optBoolean("include_unlisted",false)) {
            JSONArray others=new JSONArray(), projects=catalog.getJSONArray("projects");
            for(int i=0;i<projects.length();i++) {
                JSONObject project=projects.getJSONObject(i);
                if(!listed.contains(project.getString("id"))) {
                    JSONObject item=new JSONObject();item.put("label",project.getString("name"));item.put("project_id",project.getString("id"));others.put(item);
                }
            }
            if(others.length()>0) {
                JSONObject section=new JSONObject();section.put("id","other");section.put("label","Other");section.put("children",others);items.put(section);
            }
        }
        String selected=catalog.getString("selected_project");
        if(items.length()==0) {
            JSONArray projects=catalog.getJSONArray("projects"); items=new JSONArray();
            for(int i=0;i<projects.length();i++) {
                JSONObject project=projects.getJSONObject(i), item=new JSONObject();
                item.put("label",project.getString("name"));item.put("project_id",project.getString("id"));items.put(item);
            }
        }
        if(!selected.equals(menuProject)) {
            menuProject=selected; menuSection=""; menuExpanded=false;
            for(int i=0;i<items.length();i++) {
                JSONObject item=items.getJSONObject(i);
                if(item.has("children") && menuContains(item,selected)) menuSection=item.getString("id");
            }
        }
        View focused=selector.getFocusedChild();
        String focusedTag=focused != null && focused.getTag()!=null ? focused.getTag().toString() : "";
        selector.removeAllViews();
        TextView identity=new TextView(this); identity.setText("fungOS");
        identity.setTextSize(ShellChrome.textSize()); identity.setTextColor(Color.parseColor(palette.getString("primary")));
        identity.setGravity(Gravity.CENTER_VERTICAL); identity.setPadding(0,0,dp(ShellChrome.barPadding()),0);
        selector.addView(identity,new LinearLayout.LayoutParams(-2,barHeight()));
        String selectedName=selected;
        JSONArray catalogProjects=catalog.getJSONArray("projects");
        for(int i=0;i<catalogProjects.length();i++) if(catalogProjects.getJSONObject(i).getString("id").equals(selected)) selectedName=catalogProjects.getJSONObject(i).getString("name");
        Button workspace=new Button(this); workspace.setTag("workspace-menu");
        workspace.setSingleLine(true);workspace.setEllipsize(android.text.TextUtils.TruncateAt.END);workspace.setMaxWidth(dp(210));
        workspace.setText(selectedName + (menuExpanded ? " ▴" : " ▾")); workspace.setTextSize(ShellChrome.textSize());
        ShellChrome.control(workspace,palette,true,true); workspace.setContentDescription((menuExpanded ? "Close" : "Open") + " workspaces. Current workspace " + selectedName);
        workspace.setOnClickListener(v -> {menuExpanded=!menuExpanded;
            try {renderWorkspaceMenu(projection);} catch(Exception error) {showStatus("Menu failed: "+error.getMessage());}});
        selector.addView(workspace,new LinearLayout.LayoutParams(-2,dp(ShellChrome.controlHeight())));
        if("workspace-menu".equals(focusedTag) || (!menuExpanded && !focusedTag.isEmpty())) workspace.requestFocus();
        if(!menuExpanded) {
            String pageName=projection.getJSONObject("document").optInt("active_page",0)==0 ? "Overview" : "";
            JSONArray pageChoices=projection.getJSONObject("document").optJSONArray("pages");
            int selectedPage=projection.getJSONObject("document").optInt("active_page",0);
            if(pageChoices!=null && selectedPage>0 && selectedPage<=pageChoices.length()) pageName=pageChoices.getJSONObject(selectedPage-1).getString("name");
            if(pageChoices!=null && pageChoices.length()>0) {
                TextView page=new TextView(this);page.setText(pageName);page.setTextSize(ShellChrome.textSize());page.setGravity(Gravity.CENTER_VERTICAL);page.setTextColor(Color.parseColor(palette.getString("text_muted")));page.setPadding(dp(ShellChrome.barPadding()),0,0,0);
                selector.addView(page,new LinearLayout.LayoutParams(-2,barHeight()));
            }
            View flexible=new View(this);selector.addView(flexible,new LinearLayout.LayoutParams(0,barHeight(),1));
            android.widget.TextClock clock=new android.widget.TextClock(this);clock.setFormat12Hour("HH:mm");clock.setFormat24Hour("HH:mm");clock.setTextSize(ShellChrome.textSize());clock.setGravity(Gravity.CENTER_VERTICAL);clock.setTextColor(Color.parseColor(palette.getString("text")));
            selector.addView(clock,new LinearLayout.LayoutParams(-2,barHeight()));
            return;
        }
        JSONArray expanded=null;
        for(int i=0;i<items.length();i++) {
            JSONObject item=items.getJSONObject(i); addMenuChoice(item,projection,focusedTag);
            if(item.optString("id").equals(menuSection)) expanded=item.optJSONArray("children");
        }
        if(expanded != null) {
            View divider=new View(this); divider.setBackgroundColor(Color.parseColor(palette.getString("border")));
            LinearLayout.LayoutParams line=new LinearLayout.LayoutParams(dp(1),barHeight()-dp(14));
            line.setMargins(dp(12),dp(7),dp(8),dp(7));selector.addView(divider,line);
            for(int i=0;i<expanded.length();i++) addMenuChoice(expanded.getJSONObject(i),projection,focusedTag);
        }
        if(!libraryEndpoint.isEmpty() && (menu.optString("library_project").isEmpty() || menu.optString("library_project").equals(selected))) {
            Button library=new Button(this);library.setText("Library");library.setTextSize(ShellChrome.textSize());
            styleControl(library,palette,libraryScreen!=null);library.setOnClickListener(v -> showLibrary());
            selector.addView(library,new LinearLayout.LayoutParams(-2,barHeight()-dp(4)));
        }
        JSONArray pages=projection.getJSONObject("document").optJSONArray("pages");
        if(pages != null && pages.length()>0) for(int i=0;i<=pages.length();i++) {
            final int page=i; Button button=new Button(this);
            button.setText(i==0 ? "Overview" : pages.getJSONObject(i-1).getString("name"));button.setTextSize(ShellChrome.textSize());
            styleControl(button,palette,i==projection.getJSONObject("document").optInt("active_page",0));
            button.setOnClickListener(v -> {
                pageRotation.defer(android.os.SystemClock.uptimeMillis(), rotationInterval);
                String error=nativeSelectPage(page);
                if(error == null || !error.isEmpty()) showStatus("View switch failed: "+error);
                else {lastProjection="";refreshWorkspace();}
            });
            button.setMinHeight(0);button.setMinimumHeight(0);
            selector.addView(button,new LinearLayout.LayoutParams(-2,barHeight()-dp(4)));
        }
    }

    private void styleControl(Button button, JSONObject palette, boolean selected) throws Exception {
        ShellChrome.control(button, palette, selected);
    }

    private void refreshWorkspace() {
        int width = frame.getWidth(), height = frame.getHeight() - barHeight();
        if (width <= 0 || height <= 0) return;
        String raw = nativeProjection(width, height);
        if (raw == null) return;
        try {
            JSONObject projection = new JSONObject(raw);
            if (projection.has("error")) { showStatus(projection.getString("error")); return; }
            if (projection.isNull("document")) return;
            currentWorkspaceId=projection.getJSONObject("document").getString("id");
            String nextProjectId=projection.getJSONObject("document").getString("project_id");
            if (!currentProjectId.isEmpty() && !currentProjectId.equals(nextProjectId)) previousProjectId=currentProjectId;
            currentProjectId=nextProjectId;
            JSONObject rotations = menuDescriptor().optJSONObject("page_rotation_ms");
            int interval = rotations == null ? 0 : rotations.optInt(currentProjectId, 0);
            rotationInterval = interval;
            long now = android.os.SystemClock.uptimeMillis();
            if (pageRotation.enter(currentWorkspaceId, now, interval)) { disposeExtraStreams(); clearPanelCache(); }
            JSONArray rotatingPages = projection.getJSONObject("document").optJSONArray("pages");
            int nextPage = pageRotation.due(now, interval, rotatingPages == null ? 0 : rotatingPages.length(), projection.getJSONObject("document").optInt("active_page", 0));
            if (nextPage >= 0) {
                String error = nativeSelectPage(nextPage);
                if (error == null || !error.isEmpty()) showStatus("View rotation failed: " + error);
                else { lastProjection = ""; return; }
            }
            if (raw.equals(lastProjection)) return;
            if(channelScreen!=null && !currentWorkspaceId.equals(channelScreenWorkspace)) closeChannels();
            workspaceMode = true;
            JSONObject palette = projection.getJSONObject("palette");
            JSONObject header = projection.optJSONObject("header");
            boolean hasHeader = header != null;
            workspaceHeader.setVisibility(hasHeader ? View.VISIBLE : View.GONE);
            int headerWidth = hasHeader ? Math.min(width / 2, dp(540)) : 0;
            workspaceHeader.setLayoutParams(new FrameLayout.LayoutParams(headerWidth, barHeight(), Gravity.TOP | Gravity.RIGHT));
            FrameLayout.LayoutParams selectorBounds = (FrameLayout.LayoutParams)workspaceSelector.getLayoutParams();
            selectorBounds.rightMargin = headerWidth;
            workspaceSelector.setLayoutParams(selectorBounds);
            if (hasHeader) {
                String state = header.getJSONObject("data").optString("state", "no_data");
                String text = header.getJSONObject("frame").optString("title", "");
                if (state.equals("no_data") || state.equals("error")) text = "Unavailable";
                else if (state.equals("stale")) text += " · Stale";
                workspaceHeader.setText(text);
                workspaceHeader.setTextColor(Color.parseColor(palette.getString("text")));
                ShellChrome.bar(workspaceHeader, palette);
            }
            String structure = structuralProjection(projection);
            if (observerView != null) observerView.palette(palette);
            if (structure.equals(lastStructure)) {
                JSONArray tiles = projection.getJSONArray("tiles");
                for (int i = 0; i < tiles.length(); i++) {
                    JSONObject surface = tiles.getJSONObject(i).getJSONObject("surface");
                    StatusListView view = statusLists.get(surface.getString("id"));
                    if (view != null) view.update(surface.getJSONObject("content").getJSONObject("spec"), palette);
                    RichCardView card = richCards.get(surface.getString("id"));
                    if (card != null) card.update(surface.getJSONObject("content").getJSONObject("spec"), palette);
                }
                lastProjection = raw;
                return;
            }
            nativeResetMapMotion();
            android.util.Log.i("CanvasRetained", "rebuild workspace=" + currentWorkspaceId);
            frame.setBackgroundColor(Color.parseColor(palette.getString("background")));
            JSONObject document = projection.getJSONObject("document");
            JSONObject backgroundSpec = document.optJSONObject("background");
            themeWallpaper.setVisibility(backgroundSpec == null || "theme".equals(backgroundSpec.optString("kind"))
                ? View.VISIBLE : View.GONE);
            background.configure(document, palette);
            ShellChrome.bar(workspaceSelector, palette);
            ShellChrome.bar(selector, palette);
            styleControl(muteButton, palette, false);
            styleControl(channelButton, palette, false);
            renderWorkspaceMenu(projection);
            View previousFocus = getCurrentFocus();
            String focusedTask = currentWorkspaceId.equals(renderedWorkspace) && previousFocus instanceof SignalView
                ? ((SignalView)previousFocus).surfaceId : "";
            JSONArray tiles = projection.getJSONArray("tiles");
            boolean retainPanels = interval >= 5000 && interval <= 300000;
            int page = projection.getJSONObject("document").optInt("active_page", 0);
            boolean crossfade = retainPanels && currentWorkspaceId.equals(renderedWorkspace) && page != renderedPage;
            java.util.Set<View> previousPanels = new java.util.HashSet<>(widgets);
            java.util.Set<String> desiredPanels = new java.util.HashSet<>();
            if (retainPanels) for (int i = 0; i < tiles.length(); i++) desiredPanels.add(panelKey(tiles.getJSONObject(i), palette));
            for (View widget : widgets) {
                if (panelCache.containsValue(widget)) {
                    if (desiredPanels.stream().anyMatch(key -> panelCache.get(key) == widget)) continue;
                    widget.animate().cancel();
                    if (crossfade) widget.animate().alpha(0).setDuration(450).withEndAction(() -> widget.setVisibility(View.GONE)).start();
                    else widget.setVisibility(View.GONE);
                    continue;
                }
                if (widget instanceof PdfSurface) ((PdfSurface)widget).dispose();
                frame.removeView(widget);
            }
            widgets.clear();
            statusLists.clear();
            richCards.clear();
            boolean hasVideo = false;
            java.util.Set<String> visibleExtraStreams = new java.util.HashSet<>();
            for (int i = 0; i < tiles.length(); i++) {
                JSONObject tile = tiles.getJSONObject(i), surface = tile.getJSONObject("surface");
                JSONObject content = surface.getJSONObject("content");
                JSONArray bounds = tile.getJSONArray("bounds");
                int x = bounds.getInt(0), y = bounds.getInt(1) + barHeight();
                int w = bounds.getInt(2), h = bounds.getInt(3);
                FrameLayout.LayoutParams geometry = new FrameLayout.LayoutParams(w, h, Gravity.TOP | Gravity.LEFT);
                geometry.leftMargin = x; geometry.topMargin = y;
                String cacheKey = retainPanels ? panelKey(tile, palette) : "";
                View cached = panelCache.get(cacheKey);
                if (cached != null) {
                    cached.animate().cancel(); cached.setVisibility(View.VISIBLE);
                    cached.setLayoutParams(geometry);
                    if (crossfade && !previousPanels.contains(cached)) {
                        cached.setAlpha(0); cached.animate().alpha(1).setDuration(450).start();
                    } else cached.setAlpha(1);
                    widgets.add(cached);
                    if (cached instanceof RichCardView) {
                        RichCardView card = (RichCardView)cached;
                        card.update(content.getJSONObject("spec"), palette);
                        richCards.put(surface.getString("id"), card);
                    }
                    continue;
                }
                int beforeWidgets = widgets.size();
                if (content.getString("kind").equals("stream") && !hasVideo && content.optJSONObject("matte") == null) {
                    String url = content.getString("url");
                    String error = nativeValidate(url);
                    if (error == null || !error.isEmpty()) throw new IllegalArgumentException("Stream rejected: " + error);
                    hasVideo = true;
                    if (channelScreen!=null && !surface.getString("id").equals(channelScreenSurface)) closeChannels();
                    video.setVisibility(channelScreen==null?View.VISIBLE:View.INVISIBLE);
                    video.setLayoutParams(geometry);
                    if(channelScreen!=null) channelScreen.setLayoutParams(new FrameLayout.LayoutParams(geometry));
                    surfaceTitle = surface.getString("title");
                    videoSurfaceId = surface.getString("id");
                    setSurfaceMuted(nativeIsMuted());
                    if (!url.equals(currentUrl)) {
                        preparedPlayer = null;
                        video.stopPlayback();
                        currentUrl = url;
                        showStatus("Connecting…");
                        video.setVideoURI(Uri.parse(url));
                    }
                } else if (content.getString("kind").equals("stream")) {
                    String id = surface.getString("id"), url = content.getString("url");
                    String error = nativeValidate(url);
                    if (error == null || !error.isEmpty()) throw new IllegalArgumentException("Stream rejected: " + error);
                    if (visibleExtraStreams.size() >= 3) throw new IllegalArgumentException("At most four video tiles are supported");
                    visibleExtraStreams.add(id);
                    StreamTileView extra = extraStreams.get(id);
                    if (extra != null && !extra.matches(url, content.optJSONObject("matte"))) { extra.dispose(); frame.removeView(extra); extraStreams.remove(id); extra = null; }
                    if (extra == null) {
                        extra = new StreamTileView(this, surface.getString("title"), url, palette, content.optJSONObject("matte"));
                        extraStreams.put(id, extra); frame.addView(extra, geometry);
                    } else extra.setLayoutParams(geometry);
                    styleControl(extra.muteControl(), palette, false);
                } else if (content.getString("kind").equals("external_surface") && content.optString("source_id").equals("canvas-workspace-demo")) {
                    WorkspaceDemoView workspace = new WorkspaceDemoView(this);
                    frame.addView(workspace, geometry); widgets.add(workspace); workspace.requestFocus();
                } else if (content.getString("kind").equals("conways")) {
                    ConwaysView life = new ConwaysView(this, palette);
                    frame.addView(life, geometry); widgets.add(life);
                } else if (content.getString("kind").equals("widget")
                    && content.getJSONObject("spec").getString("kind").equals("rich_card")) {
                    RichCardView card = new RichCardView(this, content.getJSONObject("spec"), palette);
                    frame.addView(card, geometry); widgets.add(card);
                    richCards.put(surface.getString("id"), card);
                } else if (content.getString("kind").equals("widget")
                    && (content.getJSONObject("spec").getString("kind").equals("status_list")
                        || content.getJSONObject("spec").getString("kind").equals("time_series"))) {
                    StatusListView list = new StatusListView(content.getJSONObject("spec"), palette);
                    frame.addView(list, geometry); widgets.add(list); statusLists.put(surface.getString("id"), list);
                } else if (content.getString("kind").equals("widget")
                    && (content.getJSONObject("spec").getString("kind").equals("boids")
                        || content.getJSONObject("spec").getString("kind").equals("aquarium"))) {
                    BoidsView flock = new BoidsView(palette, content.getJSONObject("spec").getString("kind").equals("aquarium"),
                        content.getJSONObject("spec").optString("footnote", ""));
                    frame.addView(flock, geometry); widgets.add(flock);
                } else if (content.getString("kind").equals("widget")
                    && content.getJSONObject("spec").getString("kind").equals("signal_view")) {
                    SignalView signal = new SignalView(surface.getString("id"), content.getJSONObject("spec"), palette);
                    frame.addView(signal, geometry); widgets.add(signal);
                } else if (content.getString("kind").equals("asset")
                    && content.getJSONObject("asset").getString("media_type").equals("application/pdf")) {
                    PdfSurface pdf = new PdfSurface(content.getJSONObject("asset"), surface.getString("title"), palette, w, h);
                    frame.addView(pdf, geometry); widgets.add(pdf);
                } else if (content.getString("kind").equals("widget")
                    && content.getJSONObject("spec").getString("kind").equals("clock")
                    && content.getJSONObject("spec").optString("mode", "digital").equals("analog")) {
                    AnalogClockView clock = new AnalogClockView(palette);
                    frame.addView(clock, geometry); widgets.add(clock);
                } else {
                    TextView widget;
                    String kind = content.getString("kind");
                    if (kind.equals("widget") && content.getJSONObject("spec").getString("kind").equals("clock")) {
                        TextClock clock = new TextClock(this);
                        clock.setFormat12Hour("h:mm:ss a"); clock.setFormat24Hour("HH:mm:ss");
                        widget = clock;
                    } else {
                        widget = new TextView(this);
                        String label = surface.getString("title");
                        if (kind.equals("scene")) {
                            JSONArray nodes = content.getJSONObject("scene").getJSONArray("nodes");
                            for (int n = 0; n < nodes.length(); n++) {
                                JSONObject node = nodes.getJSONObject(n).getJSONObject("content");
                                if (node.getString("kind").equals("text")) label += "\n" + node.getString("text");
                            }
                        } else label += "\nUnsupported on TV: " + kind;
                        widget.setText(label);
                    }
                    widget.setTextSize(widget instanceof TextClock ? 28 : 18);
                    widget.setTextColor(Color.parseColor(palette.getString("text")));
                    widget.setBackgroundColor(Color.parseColor(palette.getString("surface")));
                    widget.setGravity(Gravity.CENTER);
                    widget.setPadding(20, 20, 20, 20);
                    frame.addView(widget, geometry); widgets.add(widget);
                }
                if (!cacheKey.isEmpty() && widgets.size() > beforeWidgets && panelCache.size() < 12) {
                    View panel = widgets.get(widgets.size() - 1);
                    panelCache.put(cacheKey, panel);
                    if (crossfade) { panel.setAlpha(0); panel.animate().alpha(1).setDuration(450).start(); }
                }
            }
            renderedWorkspace = currentWorkspaceId;
            renderedPage = page;
            for (String id : new java.util.ArrayList<>(extraStreams.keySet())) if (!visibleExtraStreams.contains(id)) {
                StreamTileView extra = extraStreams.remove(id); extra.dispose(); frame.removeView(extra);
            }
            if (!hasVideo) {
                closeChannels();
                videoSurfaceId=""; channelButton.setVisibility(View.GONE);
                video.stopPlayback(); preparedPlayer = null; currentUrl = "";
                video.setVisibility(View.INVISIBLE); muteButton.setVisibility(View.GONE);
                status.setVisibility(View.GONE);
            } else {
                channelButton.setVisibility(tunerWorkspace() ? View.VISIBLE : View.GONE);
                channelButton.bringToFront();
                muteButton.setVisibility(View.VISIBLE);
                FrameLayout.LayoutParams chrome = new FrameLayout.LayoutParams(dp(ShellChrome.AUDIO_WIDTH_DP), dp(ShellChrome.AUDIO_HEIGHT_DP), Gravity.TOP | Gravity.RIGHT);
                FrameLayout.LayoutParams videoBounds = (FrameLayout.LayoutParams)video.getLayoutParams();
                chrome.topMargin = videoBounds.topMargin + dp(ShellChrome.AUDIO_INSET_DP);
                chrome.rightMargin = Math.max(0, width - videoBounds.leftMargin - videoBounds.width) + dp(ShellChrome.AUDIO_INSET_DP);
                chrome.width = Math.min(chrome.width,Math.max(1,videoBounds.width-2*dp(ShellChrome.AUDIO_INSET_DP)));
                muteButton.setMaxWidth(videoBounds.width);
                muteButton.setLayoutParams(chrome);
                muteButton.bringToFront();
                if (preparedPlayer != null) status.setVisibility(View.GONE);
            }
            selector.getParent().requestLayout();
            if (!focusedTask.isEmpty()) for (View widget: widgets) {
                if (widget instanceof SignalView && ((SignalView)widget).surfaceId.equals(focusedTask)) {
                    widget.requestFocus(); break;
                }
            }
            if(libraryScreen!=null) libraryScreen.bringToFront();
            lastProjection = raw;
            lastStructure = structure;
        } catch (Exception error) { showStatus("Workspace presentation failed: " + error.getMessage()); }
    }

    private int barHeight() {
        return dp(ShellChrome.barHeight());
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }
}
