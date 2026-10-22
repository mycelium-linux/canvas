package dev.fpl.canvas;

import android.content.Context;
import android.graphics.*;
import android.graphics.drawable.GradientDrawable;
import android.view.Gravity;
import android.view.View;
import android.widget.*;
import org.json.JSONObject;

/** Platform vector fallback; GFX owns crate/disc geometry and the GPU material. */
final class RecordTile extends FrameLayout {
    private static native float[] vinylRadii(int diameter);
    private final FrameLayout sleeve;
    private final ImageView cover;
    private final TextView fallback;
    private final Vinyl disc;
    private final float withdrawn;
    private final int accent;
    private LinearLayout labels;
    RecordTile(Context host,JSONObject geometry,String name,String subtitle, int foreground,int muted,int surface,int accent) throws Exception {
        super(host);this.accent=accent;
        setId(View.generateViewId());setFocusable(true);setFocusableInTouchMode(true);setClickable(true);setSoundEffectsEnabled(false);setClipChildren(false);setClipToPadding(false);
        JSONObject bounds=geometry.getJSONObject("bounds"),paper=geometry.getJSONObject("sleeve"),record=geometry.getJSONObject("record");
        float x=(float)bounds.getDouble("x"),y=(float)bounds.getDouble("y");
        withdrawn=dp((float)(record.getDouble("x")-paper.getDouble("x")));
        disc=new Vinyl(host,accent);addView(disc,placement(record,x,y));disc.setTranslationX(-withdrawn*0.78f);
        sleeve=new FrameLayout(host);sleeve.setPadding(dp(4),dp(4),dp(4),dp(4));
        GradientDrawable stock=new GradientDrawable(GradientDrawable.Orientation.TL_BR,new int[]{0xffe1d6be,0xff827c70});stock.setCornerRadius(dp(2));sleeve.setBackground(stock);sleeve.setElevation(dp(5));
        cover=new ImageView(host);cover.setScaleType(ImageView.ScaleType.CENTER_CROP);sleeve.addView(cover,new LayoutParams(-1,-1));
        fallback=new TextView(host);fallback.setGravity(Gravity.CENTER);fallback.setText(name.isEmpty()?"◎":new String(Character.toChars(name.codePointAt(0))).toUpperCase());fallback.setTextSize(52);fallback.setTextColor(foreground);
        GradientDrawable design=new GradientDrawable(GradientDrawable.Orientation.TL_BR,new int[]{surface,accent});fallback.setBackground(design);sleeve.addView(fallback,new LayoutParams(-1,-1));
        addView(sleeve,placement(paper,x,y));
        labels=new LinearLayout(host);labels.setOrientation(LinearLayout.VERTICAL);labels.setPadding(dp(2),dp(8),dp(2),0);
        TextView title=label(host,name,18,foreground),by=label(host,subtitle,13,muted);labels.addView(title);labels.addView(by);
        LayoutParams caption=new LayoutParams(-1,dp(68));caption.topMargin=dp((float)(paper.getDouble("y")-y+paper.getDouble("height")));addView(labels,caption);
        setContentDescription(name+(subtitle.isEmpty()?"":" · "+subtitle));
    }
    private TextView label(Context context,String text,int size,int color){TextView label=new TextView(context);label.setText(text);label.setTextSize(size);label.setTextColor(color);label.setMaxLines(1);label.setEllipsize(android.text.TextUtils.TruncateAt.END);return label;}
    private int dp(float value){return Math.round(value*getResources().getDisplayMetrics().density);}
    private LayoutParams placement(JSONObject r,float x,float y)throws Exception {LayoutParams p=new LayoutParams(dp((float)r.getDouble("width")),dp((float)r.getDouble("height")));p.leftMargin=dp((float)r.getDouble("x")-x);p.topMargin=dp((float)r.getDouble("y")-y);return p;}
    void artwork(Bitmap bitmap,int color){cover.setImageBitmap(bitmap);fallback.setVisibility(GONE);disc.tint(color==0?accent:color);}
    void compact(boolean enabled){labels.setVisibility(enabled?GONE:VISIBLE);}
    void focused(boolean value){TvMotion.pose(disc,value?0:-withdrawn*0.78f,value?9:0);TvMotion.pose(sleeve,0,value?-2:0);TvMotion.focus(this,value);sleeve.setElevation(dp(value?10:5));}
    private static final class Vinyl extends View {
        private final Paint paint=new Paint(Paint.ANTI_ALIAS_FLAG);
        private float[] radii;
        private Shader reflection;
        private int tint;
        Vinyl(Context host,int color){super(host);tint=color;setLayerType(View.LAYER_TYPE_HARDWARE,null);}
        void tint(int color){tint=color;invalidate();}
        @Override protected void onSizeChanged(int w,int h,int oldW,int oldH){radii=vinylRadii(Math.min(w,h));reflection=new SweepGradient(w*.5f,h*.5f,new int[]{0xff111214,0xff393b3f,0xff101113,0xff08090b,0xff34363a,0xff0e0f11,0xff111214},null);}
        @Override protected void onDraw(Canvas canvas){if(radii==null)return;float x=getWidth()*.5f,y=getHeight()*.5f;
            paint.setStyle(Paint.Style.FILL);paint.setShader(reflection);canvas.drawCircle(x,y,radii[0],paint);paint.setShader(null);
            paint.setStyle(Paint.Style.STROKE);paint.setStrokeWidth(.7f);paint.setColor(0x554c4e52);for(int i=3;i<radii.length;i++)canvas.drawCircle(x,y,radii[i],paint);
            paint.setStyle(Paint.Style.FILL);paint.setColor(tint);canvas.drawCircle(x,y,radii[1],paint);
            paint.setColor(0x88f0e7d0);paint.setStyle(Paint.Style.STROKE);paint.setStrokeWidth(1);canvas.drawCircle(x,y,radii[1]*.82f,paint);
            paint.setStyle(Paint.Style.FILL);paint.setXfermode(new PorterDuffXfermode(PorterDuff.Mode.CLEAR));canvas.drawCircle(x,y,radii[2],paint);paint.setXfermode(null);
        }
    }
}
