/* Link-time stub for Kindle liblipc.
 *
 * The real liblipc.so.1 ships on the device. This stub only exists so the
 * kindlehf linker can resolve -llipc; DT_NEEDED is liblipc.so.1, which the
 * Kindle loads at runtime.
 */
void *LipcOpenEx(const char *service, int *code) {
    (void)service;
    (void)code;
    return 0;
}

int LipcClose(void *handle) {
    (void)handle;
    return 0;
}

int LipcRegisterStringProperty(void *lipc, const char *prop, void *getter, void *setter, void *data) {
    (void)lipc;
    (void)prop;
    (void)getter;
    (void)setter;
    (void)data;
    return 0;
}
