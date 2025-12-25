package com.margelo.nitro.http_server;

import androidx.annotation.NonNull;
import com.facebook.react.ReactPackage;
import com.facebook.react.bridge.NativeModule;
import com.facebook.react.bridge.ReactApplicationContext;
import com.facebook.react.uimanager.ViewManager;
import java.util.Collections;
import java.util.List;
import android.util.Log;

public class NitroHttpServerPackage implements ReactPackage {
    static {
        try {
            System.loadLibrary("RNHttpServer");
        } catch (Throwable e) {
            Log.e("NitroHttpServerPackage", "Failed to load RNHttpServer library", e);
        }
    }

    @NonNull
    @Override
    public List<NativeModule> createNativeModules(@NonNull ReactApplicationContext reactContext) {
        return Collections.emptyList();
    }

    @NonNull
    @Override
    public List<ViewManager> createViewManagers(@NonNull ReactApplicationContext reactContext) {
        return Collections.emptyList();
    }
}
