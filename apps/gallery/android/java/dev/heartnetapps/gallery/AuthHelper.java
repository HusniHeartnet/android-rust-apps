package dev.heartnetapps.gallery;

import android.app.Activity;
import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.os.CancellationSignal;

/**
 * Asks for the phone's own fingerprint, face or screen lock (PIN, pattern, password) through the
 * system prompt. The answer is collected by the Rust side with take().
 */
public final class AuthHelper {
    private AuthHelper() { }

    // 0 nothing yet, 1 confirmed, 2 cancelled or failed, 3 the phone has no screen lock to ask for
    private static volatile int result = 0;

    public static void authenticate(final Activity a, final String title) {
        result = 0;
        a.runOnUiThread(new Runnable() {
            @Override public void run() {
                try {
                    BiometricPrompt prompt = new BiometricPrompt.Builder(a)
                            .setTitle(title)
                            .setSubtitle("Confirm it is you")
                            .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG
                                    | BiometricManager.Authenticators.DEVICE_CREDENTIAL)
                            .build();
                    prompt.authenticate(new CancellationSignal(), a.getMainExecutor(), new BiometricPrompt.AuthenticationCallback() {
                        @Override public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult r) {
                            result = 1;
                        }

                        @Override public void onAuthenticationError(int code, CharSequence message) {
                            // 11 no biometrics enrolled, 12 no hardware, 14 no screen lock set
                            result = (code == 11 || code == 12 || code == 14) ? 3 : 2;
                        }
                    });
                } catch (Throwable t) {
                    result = 3;
                }
            }
        });
    }

    /** The answer, once, or 0 while the prompt is still up. */
    public static int take() {
        int r = result;
        if (r != 0) result = 0;
        return r;
    }
}
