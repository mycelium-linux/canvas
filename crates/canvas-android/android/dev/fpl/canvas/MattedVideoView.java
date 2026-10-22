package dev.fpl.canvas;

import android.content.Context;
import android.graphics.SurfaceTexture;
import android.media.MediaPlayer;
import android.net.Uri;
import android.opengl.EGL14;
import android.opengl.EGLConfig;
import android.opengl.EGLContext;
import android.opengl.EGLDisplay;
import android.opengl.EGLSurface;
import android.opengl.GLES11Ext;
import android.opengl.GLES20;
import android.os.Handler;
import android.os.HandlerThread;
import android.util.Log;
import android.view.Surface;
import android.view.TextureView;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;
import org.json.JSONArray;
import org.json.JSONObject;

/** Platform decoding/lifecycle only. GFX owns matte contract and shaders. */
final class MattedVideoView extends TextureView implements TextureView.SurfaceTextureListener {
    interface Listener { void prepared(); void audioTracks(int availability); void failed(String message); }
    private static native String nativeShaders();
    private static EGLDisplay sharedDisplay = EGL14.EGL_NO_DISPLAY;
    private static int displayUsers;
    private static synchronized EGLDisplay acquireDisplay() {
        if (displayUsers == 0) {
            sharedDisplay = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY);
            int[] version = new int[2];
            require(EGL14.eglInitialize(sharedDisplay, version, 0, version, 1), "EGL initialize");
        }
        displayUsers++;
        return sharedDisplay;
    }
    private static synchronized void releaseDisplay() {
        if (--displayUsers == 0) { EGL14.eglTerminate(sharedDisplay); sharedDisplay = EGL14.EGL_NO_DISPLAY; }
    }
    private final HandlerThread thread = new HandlerThread("Canvas video matte");
    private final Handler worker;
    private final Listener listener;
    private final String url;
    private final JSONObject matte;
    private final String vertexShader, fragmentShader;
    private volatile boolean disposed, muted = true;
    private Renderer renderer;

    MattedVideoView(Context context, String url, JSONObject matte, Listener listener) throws Exception {
        super(context);
        this.url = url; this.matte = matte; this.listener = listener;
        JSONObject shaders = new JSONObject(nativeShaders());
        vertexShader = shaders.getString("vertex"); fragmentShader = shaders.getString("fragment");
        setOpaque(false);
        thread.start(); worker = new Handler(thread.getLooper());
        setSurfaceTextureListener(this);
    }
    void setMuted(boolean value) {
        muted = value;
        worker.post(() -> { if (renderer != null && renderer.player != null) renderer.player.setVolume(value ? 0 : 1, value ? 0 : 1); });
    }
    void dispose() {
        if (disposed) return;
        disposed = true;
        worker.post(() -> { if (renderer != null) { renderer.release(); renderer = null; } thread.quitSafely(); });
    }
    private void failure(Exception error) {
        Log.e("CanvasVideoMatte", "GPU video presentation failed", error);
        post(() -> { if (!disposed) listener.failed("Video matte failed: " + error.getMessage()); });
    }
    @Override public void onSurfaceTextureAvailable(SurfaceTexture surface, int width, int height) {
        worker.post(() -> {
            if (disposed) return;
            try {
                renderer = new Renderer(surface, width, height);
                renderer.start();
            } catch (Exception error) {
                if (renderer != null) { renderer.release(); renderer = null; }
                failure(error);
            }
        });
    }
    @Override public void onSurfaceTextureSizeChanged(SurfaceTexture surface, int width, int height) {
        worker.post(() -> { if (renderer != null) { renderer.width = width; renderer.height = height; renderer.draw(false); } });
    }
    @Override public boolean onSurfaceTextureDestroyed(SurfaceTexture surface) {
        if (!worker.post(() -> { if (renderer != null) { renderer.release(); renderer = null; } surface.release(); })) surface.release();
        return false; // release after EGL has stopped using this output
    }
    @Override public void onSurfaceTextureUpdated(SurfaceTexture surface) {}

    private final class Renderer {
        private EGLDisplay display = EGL14.EGL_NO_DISPLAY;
        private EGLContext context = EGL14.EGL_NO_CONTEXT;
        private EGLSurface window = EGL14.EGL_NO_SURFACE;
        private final SurfaceTexture output;
        private Surface outputSurface, decoderSurface;
        private SurfaceTexture decoded;
        private MediaPlayer player;
        private int width, height, program, texture, sourceWidth, sourceHeight;
        private boolean frameReady, stopped;
        private final float[] transform = new float[16];
        private final FloatBuffer positions = floats(new float[]{-1,-1, 1,-1, -1,1, 1,1});
        private final FloatBuffer uvs = floats(new float[]{0,0, 1,0, 0,1, 1,1});
        Renderer(SurfaceTexture output, int width, int height) { this.output = output; this.width = width; this.height = height; }
        void start() throws Exception {
            display = acquireDisplay();
            int[] configAttributes = {EGL14.EGL_RED_SIZE,8, EGL14.EGL_GREEN_SIZE,8, EGL14.EGL_BLUE_SIZE,8,
                EGL14.EGL_ALPHA_SIZE,8, EGL14.EGL_RENDERABLE_TYPE,EGL14.EGL_OPENGL_ES2_BIT,
                EGL14.EGL_SURFACE_TYPE,EGL14.EGL_WINDOW_BIT, EGL14.EGL_NONE};
            EGLConfig[] configs = new EGLConfig[1]; int[] count = new int[1];
            require(EGL14.eglChooseConfig(display, configAttributes, 0, configs, 0, 1, count, 0) && count[0] > 0, "RGBA EGL config");
            context = EGL14.eglCreateContext(display, configs[0], EGL14.EGL_NO_CONTEXT,
                new int[]{EGL14.EGL_CONTEXT_CLIENT_VERSION,2,EGL14.EGL_NONE}, 0);
            require(context != EGL14.EGL_NO_CONTEXT, "EGL context");
            outputSurface = new Surface(output);
            window = EGL14.eglCreateWindowSurface(display, configs[0], outputSurface, new int[]{EGL14.EGL_NONE}, 0);
            require(window != EGL14.EGL_NO_SURFACE, "EGL window");
            require(EGL14.eglMakeCurrent(display, window, window, context), "EGL current");
            int vertex = shader(GLES20.GL_VERTEX_SHADER, vertexShader);
            int fragment = shader(GLES20.GL_FRAGMENT_SHADER, fragmentShader);
            program = GLES20.glCreateProgram(); GLES20.glAttachShader(program, vertex); GLES20.glAttachShader(program, fragment);
            GLES20.glLinkProgram(program); int[] linked = new int[1]; GLES20.glGetProgramiv(program, GLES20.GL_LINK_STATUS, linked, 0);
            GLES20.glDeleteShader(vertex); GLES20.glDeleteShader(fragment);
            require(linked[0] != 0, "GFX video shader link: " + GLES20.glGetProgramInfoLog(program));
            GLES20.glUseProgram(program);
            GLES20.glUniform1f(uniform("uPackedAlpha"), matte.optBoolean("packed_alpha", false) ? 1f : 0f);
            JSONArray regions = matte.getJSONArray("regions");
            GLES20.glUniform1i(uniform("uRegionCount"), regions.length());
            for (int i=0; i<regions.length(); i++) {
                JSONObject region = regions.getJSONObject(i); JSONArray bounds = region.getJSONArray("bounds"), color = region.getJSONArray("color");
                GLES20.glUniform4f(uniform("uBounds["+i+"]"), (float)bounds.getDouble(0), (float)bounds.getDouble(1), (float)bounds.getDouble(2), (float)bounds.getDouble(3));
                GLES20.glUniform3f(uniform("uColor["+i+"]"), color.getInt(0)/255f, color.getInt(1)/255f, color.getInt(2)/255f);
                GLES20.glUniform2f(uniform("uRamp["+i+"]"), (float)region.getDouble("cutoff"), (float)region.getDouble("feather"));
            }
            int[] textures = new int[1]; GLES20.glGenTextures(1, textures, 0); texture = textures[0];
            GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, texture);
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR);
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR);
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE);
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE);
            decoded = new SurfaceTexture(texture);
            decoded.setOnFrameAvailableListener(frame -> draw(true), worker);
            decoderSurface = new Surface(decoded);
            player = new MediaPlayer(); player.setSurface(decoderSurface);
            player.setOnVideoSizeChangedListener((p, w, h) -> checkSize(w, h));
            player.setOnPreparedListener(p -> {
                if (stopped || disposed) return;
                if (!checkSize(p.getVideoWidth(), p.getVideoHeight())) return;
                publishAudioTracks();
                p.setVolume(muted ? 0 : 1, muted ? 0 : 1); p.start();
                post(() -> { if (!disposed) listener.prepared(); });
            });
            player.setOnInfoListener((p,what,extra) -> {
                if(!stopped && !disposed && (what==MediaPlayer.MEDIA_INFO_METADATA_UPDATE || what==MediaPlayer.MEDIA_INFO_VIDEO_RENDERING_START)) publishAudioTracks();
                return false;
            });
            player.setOnErrorListener((p, what, extra) -> { stopWithError("Decoder error " + what + "/" + extra); return true; });
            player.setOnCompletionListener(p -> stopWithError("Stream ended"));
            player.setDataSource(getContext(), Uri.parse(url)); player.prepareAsync();
            Log.i("CanvasVideoMatte", "GFX shader linked; RGBA TextureView active");
        }
        private boolean checkSize(int w, int h) {
            try {
                if (w == 0 || h == 0) return false;
                JSONArray size = matte.getJSONArray("source_size");
                if (w != size.getInt(0) || h != size.getInt(1)) {
                    stopWithError("Decoded dimensions " + w + "x" + h + " differ from matte source"); return false;
                }
                sourceWidth = matte.optBoolean("packed_alpha", false) ? w / 2 : w;
                sourceHeight = h; return true;
            } catch (Exception error) { failure(error); release(); return false; }
        }
        private void publishAudioTracks() {
            int availability=DecoderAudio.inspect(player);
            post(() -> { if(!disposed) listener.audioTracks(availability); });
        }
        private void stopWithError(String message) {
            post(() -> { if (!disposed) listener.failed(message); }); release();
        }
        void draw(boolean update) {
            if (stopped || disposed || sourceWidth == 0 || sourceHeight == 0) return;
            try {
                if (update) { decoded.updateTexImage(); decoded.getTransformMatrix(transform); frameReady = true; }
                if (!frameReady) return;
                GLES20.glViewport(0, 0, width, height);
                GLES20.glClearColor(0,0,0,0); GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT);
                float scale = Math.min((float)width/sourceWidth, (float)height/sourceHeight);
                int fittedW = Math.round(sourceWidth * scale), fittedH = Math.round(sourceHeight * scale);
                GLES20.glViewport((width-fittedW)/2, (height-fittedH)/2, fittedW, fittedH);
                GLES20.glUseProgram(program);
                attribute("aPosition", positions); attribute("aUv", uvs);
                GLES20.glUniformMatrix4fv(uniform("uTextureMatrix"), 1, false, transform, 0);
                GLES20.glActiveTexture(GLES20.GL_TEXTURE0); GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, texture);
                GLES20.glUniform1i(uniform("uFrame"), 0);
                GLES20.glDrawArrays(GLES20.GL_TRIANGLE_STRIP, 0, 4);
                int error = GLES20.glGetError(); require(error == GLES20.GL_NO_ERROR, "GLES draw " + error);
                require(EGL14.eglSwapBuffers(display, window), "EGL swap");
            } catch (Exception error) { failure(error); release(); }
        }
        private void attribute(String name, FloatBuffer values) {
            int location = GLES20.glGetAttribLocation(program, name); values.position(0);
            GLES20.glEnableVertexAttribArray(location); GLES20.glVertexAttribPointer(location, 2, GLES20.GL_FLOAT, false, 0, values);
        }
        private int uniform(String name) { return GLES20.glGetUniformLocation(program, name); }
        void release() {
            if (stopped) return; stopped = true;
            if (player != null) { player.release(); player = null; }
            if (decoderSurface != null) decoderSurface.release();
            if (decoded != null) { decoded.setOnFrameAvailableListener(null); decoded.release(); }
            if (display != EGL14.EGL_NO_DISPLAY) {
                if (program != 0) GLES20.glDeleteProgram(program);
                if (texture != 0) GLES20.glDeleteTextures(1, new int[]{texture}, 0);
                EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT);
                if (window != EGL14.EGL_NO_SURFACE) EGL14.eglDestroySurface(display, window);
                if (context != EGL14.EGL_NO_CONTEXT) EGL14.eglDestroyContext(display, context);
                releaseDisplay(); EGL14.eglReleaseThread();
            }
            if (outputSurface != null) outputSurface.release();
        }
    }
    private static FloatBuffer floats(float[] values) { FloatBuffer b = ByteBuffer.allocateDirect(values.length*4).order(ByteOrder.nativeOrder()).asFloatBuffer(); b.put(values).position(0); return b; }
    private static int shader(int kind, String source) {
        int shader = GLES20.glCreateShader(kind); GLES20.glShaderSource(shader, source); GLES20.glCompileShader(shader);
        int[] status = new int[1]; GLES20.glGetShaderiv(shader, GLES20.GL_COMPILE_STATUS, status, 0);
        if (status[0] == 0) { String message = GLES20.glGetShaderInfoLog(shader); GLES20.glDeleteShader(shader); throw new IllegalStateException("GFX shader compile: " + message); }
        return shader;
    }
    private static void require(boolean ok, String message) { if (!ok) throw new IllegalStateException(message); }
}
