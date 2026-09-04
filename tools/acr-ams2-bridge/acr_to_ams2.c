#define WIN32_LEAN_AND_MEAN
#define _WIN32_WINNT 0x0600

#include <winsock2.h>
#include <windows.h>

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define ACR_PHYSICS_MAPPING "Local\\acpmf_physics"

#define AMS2_PACKET_SIZE 559
#define AMS2_PORT 5606

#define SEND_INTERVAL_MS 10
#define TELEMETRY_FRESH_MS 1000

/*
 * AC Rally uses the ACC-style SPageFilePhysics layout.
 *
 * Relevant offsets:
 *
 *   packetId       0   int32
 *   rpm           20   int32
 *   currentMaxRpm 588  int32
 *
 * We only describe enough of the structure to reach currentMaxRpm.
 */
#pragma pack(push, 4)
typedef struct {
    int32_t packet_id;             /* offset   0 */

    float gas;                     /* offset   4 */
    float brake;                   /* offset   8 */
    float fuel;                    /* offset  12 */
    int32_t gear;                  /* offset  16 */
    int32_t rpm;                   /* offset  20 */

    uint8_t unused_24_to_587[588 - 24];

    int32_t current_max_rpm;       /* offset 588 */
} acr_physics;
#pragma pack(pop)

_Static_assert(offsetof(acr_physics, rpm) == 20,
               "Unexpected ACR rpm offset");

_Static_assert(offsetof(acr_physics, current_max_rpm) == 588,
               "Unexpected ACR currentMaxRpm offset");

static volatile LONG keep_running = 1;

static BOOL WINAPI console_handler(DWORD event)
{
    switch (event) {
    case CTRL_C_EVENT:
    case CTRL_BREAK_EVENT:
    case CTRL_CLOSE_EVENT:
        InterlockedExchange(&keep_running, 0);
        return TRUE;

    default:
        return FALSE;
    }
}

static uint16_t rpm_to_u16(int32_t rpm)
{
    if (rpm <= 0)
        return 0;

    if (rpm >= 65535)
        return UINT16_MAX;

    return (uint16_t)rpm;
}

static void put_u16_le(uint8_t *destination, uint16_t value)
{
    destination[0] = (uint8_t)(value & 0xffu);
    destination[1] = (uint8_t)((value >> 8) & 0xffu);
}

static void put_u32_le(uint8_t *destination, uint32_t value)
{
    destination[0] = (uint8_t)(value & 0xffu);
    destination[1] = (uint8_t)((value >> 8) & 0xffu);
    destination[2] = (uint8_t)((value >> 16) & 0xffu);
    destination[3] = (uint8_t)((value >> 24) & 0xffu);
}

static void build_ams2_packet(
    uint8_t packet[AMS2_PACKET_SIZE],
    uint32_t sequence,
    uint16_t rpm,
    uint16_t maximum_rpm)
{
    memset(packet, 0, AMS2_PACKET_SIZE);

    /*
     * Project CARS 2 UDP PacketBase:
     *
     *   0: packet number
     *   4: category packet number
     *   8: partial packet index
     *   9: number of partial packets
     *  10: packet type, 0 = car physics
     *  11: packet version, 2
     */
    put_u32_le(packet + 0, sequence);
    put_u32_le(packet + 4, sequence);

    packet[8] = 0;
    packet[9] = 1;
    packet[10] = 0;
    packet[11] = 2;

    /* Viewed participant index. */
    packet[12] = 0;

    /* sTelemetryData::sRpm and sMaxRpm. */
    put_u16_le(packet + 40, rpm);
    put_u16_le(packet + 42, maximum_rpm);
}

static void print_windows_error(const char *operation)
{
    fprintf(
        stderr,
        "%s failed with Windows error %lu\n",
        operation,
        (unsigned long)GetLastError()
    );
}

int main(void)
{
    WSADATA winsock_data;
    SOCKET socket_handle = INVALID_SOCKET;
    struct sockaddr_in destination;

    HANDLE mapping = NULL;
    volatile const acr_physics *shared = NULL;

    uint8_t packet[AMS2_PACKET_SIZE];
    uint32_t sequence = 0;

    int32_t last_packet_id;
    ULONGLONG last_packet_change;
    ULONGLONG last_status;

    SetConsoleCtrlHandler(console_handler, TRUE);

    if (WSAStartup(MAKEWORD(2, 2), &winsock_data) != 0) {
        fprintf(stderr, "WSAStartup failed\n");
        return 1;
    }

    socket_handle = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);

    if (socket_handle == INVALID_SOCKET) {
        fprintf(
            stderr,
            "socket failed with Winsock error %d\n",
            WSAGetLastError()
        );
        WSACleanup();
        return 1;
    }

    memset(&destination, 0, sizeof(destination));
    destination.sin_family = AF_INET;
    destination.sin_port = htons(AMS2_PORT);
    destination.sin_addr.s_addr = htonl(INADDR_LOOPBACK);

    mapping = OpenFileMappingA(
        FILE_MAP_READ,
        FALSE,
        ACR_PHYSICS_MAPPING
    );

    if (mapping == NULL) {
        print_windows_error("OpenFileMappingA");
        fprintf(
            stderr,
            "Assetto Corsa Rally telemetry mapping is not available.\n"
        );
        closesocket(socket_handle);
        WSACleanup();
        return 1;
    }

    shared = (volatile const acr_physics *)MapViewOfFile(
        mapping,
        FILE_MAP_READ,
        0,
        0,
        sizeof(acr_physics)
    );

    if (shared == NULL) {
        print_windows_error("MapViewOfFile");
        CloseHandle(mapping);
        closesocket(socket_handle);
        WSACleanup();
        return 1;
    }

    printf(
        "Assetto Corsa Rally shared memory connected: %s\n",
        ACR_PHYSICS_MAPPING
    );

    printf(
        "Sending Project CARS 2 telemetry to 127.0.0.1:%d\n",
        AMS2_PORT
    );

    printf("RPM reference: AC Rally currentMaxRpm\n");

    last_packet_id = shared->packet_id;
    last_packet_change = GetTickCount64();
    last_status = 0;

    while (InterlockedCompareExchange(&keep_running, 1, 1)) {
        ULONGLONG now = GetTickCount64();

        int32_t packet_id = shared->packet_id;
        int32_t raw_rpm = shared->rpm;
        int32_t raw_max_rpm = shared->current_max_rpm;

        uint16_t rpm;
        uint16_t maximum_rpm;

        int telemetry_fresh;
        int driving;

        if (packet_id != last_packet_id) {
            last_packet_id = packet_id;
            last_packet_change = now;
        }

        telemetry_fresh =
            now - last_packet_change <= TELEMETRY_FRESH_MS;

        /*
         * A valid currentMaxRpm is also a convenient guard against
         * startup/menu garbage or attaching before live physics exists.
         */
        driving =
            telemetry_fresh &&
            raw_max_rpm > 1000 &&
            raw_max_rpm < 65536 &&
            raw_rpm >= 0;

        rpm = rpm_to_u16(raw_rpm);
        maximum_rpm = rpm_to_u16(raw_max_rpm);

        if (driving) {
            int result;

            build_ams2_packet(
                packet,
                sequence++,
                rpm,
                maximum_rpm
            );

            result = sendto(
                socket_handle,
                (const char *)packet,
                sizeof(packet),
                0,
                (const struct sockaddr *)&destination,
                sizeof(destination)
            );

            if (result == SOCKET_ERROR) {
                fprintf(
                    stderr,
                    "sendto failed with Winsock error %d\n",
                    WSAGetLastError()
                );
            }
        }

        if (now - last_status >= 1000) {
            if (driving) {
                printf(
                    "packet=%-8ld rpm %5u/%-5u sequence=%lu\n",
                    (long)packet_id,
                    (unsigned)rpm,
                    (unsigned)maximum_rpm,
                    (unsigned long)sequence
                );
            } else {
                printf(
                    "Waiting for live AC Rally physics "
                    "(packet=%ld rpm=%ld max=%ld)\n",
                    (long)packet_id,
                    (long)raw_rpm,
                    (long)raw_max_rpm
                );
            }

            last_status = now;
        }

        Sleep(SEND_INTERVAL_MS);
    }

    printf("Stopping bridge\n");

    UnmapViewOfFile((const void *)shared);
    CloseHandle(mapping);
    closesocket(socket_handle);
    WSACleanup();

    return 0;
}
