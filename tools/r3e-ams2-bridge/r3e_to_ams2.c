#define WIN32_LEAN_AND_MEAN
#define _WIN32_WINNT 0x0600

#include <winsock2.h>
#include <windows.h>

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "r3e.h"

#define AMS2_PACKET_SIZE 559
#define AMS2_PORT 5606
#define SEND_INTERVAL_MS 10
#define TELEMETRY_FRESH_MS 1000

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

static uint16_t rps_to_rpm(float rps)
{
    double rpm;

    if (!isfinite(rps) || rps <= 0.0f)
        return 0;

    rpm = (double)rps * 60.0 / (2.0 * 3.14159265358979323846);

    if (rpm >= 65535.0)
        return UINT16_MAX;

    return (uint16_t)(rpm + 0.5);
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

int main(int argc, char **argv)
{
    int use_upshift = 0;
    WSADATA winsock_data;
    SOCKET socket_handle = INVALID_SOCKET;
    struct sockaddr_in destination;

    HANDLE mapping = NULL;
    volatile const r3e_shared *shared = NULL;

    uint8_t packet[AMS2_PACKET_SIZE];
    uint32_t sequence = 0;

    int32_t last_simulation_ticks;
    ULONGLONG last_tick_change;
    ULONGLONG last_status;

    if (argc == 2 && strcmp(argv[1], "--upshift") == 0) {
        use_upshift = 1;
    } else if (argc != 1) {
        fprintf(stderr, "Usage: r3e_to_ams2.exe [--upshift]\n");
        return 2;
    }

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
        R3E_SHARED_MEMORY_NAME
    );

    if (mapping == NULL) {
        print_windows_error("OpenFileMappingA");
        fprintf(
            stderr,
            "RaceRoom must already be running in App ID 211500.\n"
        );
        closesocket(socket_handle);
        WSACleanup();
        return 1;
    }

    shared = (volatile const r3e_shared *)MapViewOfFile(
        mapping,
        FILE_MAP_READ,
        0,
        0,
        sizeof(r3e_shared)
    );

    if (shared == NULL) {
        print_windows_error("MapViewOfFile");
        CloseHandle(mapping);
        closesocket(socket_handle);
        WSACleanup();
        return 1;
    }

    printf(
        "RaceRoom shared-memory API: %d.%d; compiled for %d.%d\n",
        shared->version_major,
        shared->version_minor,
        R3E_VERSION_MAJOR,
        R3E_VERSION_MINOR
    );

    if (shared->version_major != R3E_VERSION_MAJOR ||
        shared->version_minor != R3E_VERSION_MINOR) {
        fprintf(
            stderr,
            "Warning: RaceRoom API version differs from downloaded r3e.h\n"
        );
    }

    printf(
        "Sending Project CARS 2 telemetry to 127.0.0.1:%d\n",
        AMS2_PORT
    );

    printf(
        "RPM reference: %s\n",
        use_upshift ? "RaceRoom upshift RPM" : "engine maximum RPM"
    );

    last_simulation_ticks = shared->player.game_simulation_ticks;
    last_tick_change = GetTickCount64();
    last_status = 0;

    while (InterlockedCompareExchange(&keep_running, 1, 1)) {
        ULONGLONG now = GetTickCount64();
        int32_t simulation_ticks =
            shared->player.game_simulation_ticks;

        float engine_rps = shared->engine_rps;
        float maximum_rps = shared->max_engine_rps;
        float upshift_rps = shared->upshift_rps;
        float reference_rps;

        uint16_t rpm;
        uint16_t reference_rpm;

        int telemetry_fresh;
        int driving;

        if (simulation_ticks != last_simulation_ticks) {
            last_simulation_ticks = simulation_ticks;
            last_tick_change = now;
        }

        telemetry_fresh =
            now - last_tick_change <= TELEMETRY_FRESH_MS;

        driving =
            telemetry_fresh &&
            shared->game_paused == 0 &&
            shared->game_in_menus == 0;

        if (use_upshift && upshift_rps > 0.0f)
            reference_rps = upshift_rps;
        else if (maximum_rps > 0.0f)
            reference_rps = maximum_rps;
        else
            reference_rps = upshift_rps;

        rpm = rps_to_rpm(engine_rps);
        reference_rpm = rps_to_rpm(reference_rps);

        if (driving && reference_rpm > 0) {
            int result;

            build_ams2_packet(
                packet,
                sequence++,
                rpm,
                reference_rpm
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
            if (driving && reference_rpm > 0) {
                printf(
                    "rpm %5u/%-5u  sequence=%lu\n",
                    (unsigned)rpm,
                    (unsigned)reference_rpm,
                    (unsigned long)sequence
                );
            } else {
                printf("Waiting for live RaceRoom physics\n");
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
