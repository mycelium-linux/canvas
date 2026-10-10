package dev.fpl.canvas;

/** JVM-only contract checks: no device volume, decoder or Android runtime needed. */
public final class AudioChromeTest {
    public static void main(String[] args) {
        if (!ShellChrome.audioAction(true).equals("Unmute")) throw new AssertionError("muted action");
        if (!ShellChrome.audioAction(false).equals("Mute")) throw new AssertionError("unmuted action");
        if (!ShellChrome.audioDescription("KOMO",true).equals("Unmute KOMO audio. This stream is muted."))
            throw new AssertionError("source-specific muted state");
        if (!ShellChrome.audioDescription("Tour",false).equals("Mute Tour audio. This stream is unmuted."))
            throw new AssertionError("source-specific unmuted state");
        if (ShellChrome.audioAction(true).length()>6) throw new AssertionError("compact label");
        if (ShellChrome.AUDIO_HEIGHT_DP<44 || ShellChrome.AUDIO_WIDTH_DP<ShellChrome.AUDIO_HEIGHT_DP)
            throw new AssertionError("minimum remote target");
        System.out.println("Audio chrome contracts passed");
    }
}
