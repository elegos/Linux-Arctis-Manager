from typing import Optional

from arctis_manager.config_manager import ConfigManager
from arctis_manager.device_manager import (DeviceState, DeviceManager,
                                           DeviceStatus, InterfaceEndpoint)
from arctis_manager.device_manager.device_settings import (DeviceSetting,
                                                           SliderSetting,
                                                           ToggleSetting)
from arctis_manager.device_manager.device_status import DeviceStatusValue

INACTIVE_TIME_MINUTES = {
    0: 0,
    1: 1,
    2: 5,
    3: 10,
    4: 15,
    5: 30,
    6: 60,
}

STATUS_REQUEST_MESSAGE = [0x06, 0xb0]


# TODO: tonitch
class ArctisNovaProWirelessDevice(DeviceManager):
    game_mix: int = None
    chat_mix: int = None

    def get_local_settings(self) -> dict[str, int]:
        '''
        Returns the set of settings that are being set during initialization.
        These settings are not been sent by the device's status messages.
        '''

        self._local_config = ConfigManager.get_instance().get_config(self.get_device_vendor_id(), self.get_device_product_id()) or {
            'wireless_mode': 0x00,  # speed
            'mic_volume': 0x0a,  # 100%
            'mic_side_tone': 0x00,  # off
            'mic_gain': 0x02,  # high
            'mic_led_brightness': 0x0a,  # 100%
            'pm_shutdown': 0x05,  # 30 minutes
        }

        return self._local_config

    def save_local_settings(self) -> None:
        ConfigManager.get_instance().save_config(self.get_device_vendor_id(), self.get_device_product_id(), self._local_config)

    def get_device_name(self):
        return 'Arctis Nova 4P Wireless'

    def get_device_product_id(self):
        return 0x12f2

    def get_endpoint_addresses_to_listen(self) -> list[InterfaceEndpoint]:
        return [self.utility_guess_endpoint(7, 'in')]

    def get_request_device_status(self):
        return self.utility_guess_endpoint(7, 'out'), STATUS_REQUEST_MESSAGE

    def init_device(self):
        '''
        Initializes the GameDAC Gen2, enabling the mixer.
        Kinda obscure, but seems to work on my machine (tm).
        (Packets and sequence taken from the Arctis Nova Pro Wireless via Wireshark)
        '''

        local_settings = self.get_local_settings()

        commands = [
            # Command, expects response

            # Series of queries / responses
            ([0x06, 0x20], True),
            ([0x06, 0x20], True),
            ([0x06, 0x10], True),
            ([0x06, 0x10], True),
            ([0x06, 0x10], True),
            ([0x06, 0x3b], False),  # Correction?
            ([0x06, 0x8d, 0x01], True),
            ([0x06, 0x20], True),
            ([0x06, 0x20], True),
            ([0x06, 0x20], True),
            ([0x06, 0x80], True),
            ([0x06, 0x3b], False),  # Correction?
            # Burst of commands (device init?)
            ([0x06, 0x8d, 0x01], False),
            ([0x06, 0x33, 0x14, 0x14, 0x14], False),  # Equalizer with 3 bands
            ([0x06, 0xc3, local_settings['wireless_mode']], False),  # 2.4G mode (0x00: speed, 0x01: range)
            ([0x06, 0x2e, 0x00], False),  # Set equalizer preset (0)
            ([0x06, 0xc1, local_settings['pm_shutdown']], False),  # Set inactive time (to 30 minutes, see INACTIVE_TIME_MINUTES)
            ([0x06, 0x85, 0x0a], False),
            ([0x06, 0x37, local_settings['mic_volume']], False),  # Mic volume 100% (01 (mute) - a0 (100%))
            ([0x06, 0xb2], False),
            ([0x06, 0x47, 0x64, 0x00, 0x64], False),
            ([0x06, 0x83, 0x01], False),
            ([0x06, 0x89, 0x00], False),
            ([0x06, 0x27, local_settings['mic_gain']], False),  # Gain (0x01: low, 0x02: high)
            ([0x06, 0xb3, 0x00], False),
            ([0x06, 0x39, local_settings['mic_side_tone']], False),  # Set the sidetone to 0 (off) -> possible values: 0 (off), 1 (low), 2 (medium), 3 (high)
            ([0x06, 0xbf, local_settings['mic_led_brightness']], False),  # Mute mic led brightness (out of 10)
            ([0x06, 0x43, 0x01], False),
            ([0x06, 0x69, 0x00], False),
            ([0x06, 0x3b, 0x00], False),
            ([0x06, 0x8d, 0x01], False),
            ([0x06, 0x49, 0x01], False),
            ([0x06, 0xb7, 0x00], False),

            # Another series of queries (perhaps for confirmation?)
            ([0x06, 0xb7, 0x00], True),
            ([0x06, 0xb7, 0x00], True),
            (STATUS_REQUEST_MESSAGE, True),  # Get device status
            ([0x06, 0x20, 0x00], True),
            ([0x06, 0xb7, 0x00], True),
        ]

        endpoint, _ = self.get_request_device_status()
        self.kernel_detach(endpoint)

        for command in commands:
            self.send_06_command(command[0], False)

    def manage_input_data(self, data: list[int], endpoint: InterfaceEndpoint) -> DeviceState:
        volume = 1
        device_status = None

        if endpoint == InterfaceEndpoint(7, 0):
            # Volume control is managed by the GameDAC
            if len(data) >= 2 and data[0] == 0x07 and data[1] == 0x25:
                # Volume is data[2]. If needed for any other purpose, it ranges between -56 (0%) and 0 (100%).
                pass

            elif len(data) >= 4 and data[0] == 0x07 and data[1] == 0x45:
                self.log.debug('Received volume control data.')
                self.game_mix = data[2] / 100  # Ranges from 0 to 100
                self.chat_mix = data[3] / 100  # Ranges from 0 to 100
            elif len(data) >= 16 and data[0] == 0x06 and data[1] == 0xb0:
                # https://github.com/Sapd/HeadsetControl/blob/master/src/devices/steelseries_arctis_nova_pro_wireless.c#L242
                device_status = DeviceStatus(
                    bluetooth_powerup_state=DeviceStatusValue(data[2], 'on_off.off' if data[2] == 0x00 else 'on_off.on'),
                    bluetooth_auto_mute=DeviceStatusValue(data[3], 'on_off.off' if data[3] == 0x00 else 'db.-12db' if data[3] == 0x01 else 'on_off.on'),
                    bluetooth_power_status=DeviceStatusValue(data[4], 'on_off.off' if data[4] == 0x01 else 'on_off.on'),
                    bluetooth_connection=DeviceStatusValue(data[5], 'on_off.off' if data[5] ==
                                                           0x00 else 'connection.connected' if data[5] == 0x01 else 'connection.disconnected'),
                    headset_battery_charge=DeviceStatusValue(data[6], mapped_val=lambda x: round(x / 8, 2)),
                    charge_slot_battery_charge=DeviceStatusValue(data[7], mapped_val=lambda x: round(x / 8, 2)),
                    transparent_noise_cancelling_level=DeviceStatusValue(data[8], mapped_val=lambda x: round(x / 10, 0)),
                    mic_status=DeviceStatusValue(data[9], 'mic_status.unmuted' if data[9] == 0x00 else 'mic_status.muted'),
                    noise_cancelling=DeviceStatusValue(data[10], 'on_off.off' if data[10] == 0x00 else 'anc.transparent' if data[10] == 0x01 else 'on_off.on'),
                    mic_led_brightness=DeviceStatusValue(data[11], mapped_val=lambda x: x / 10),
                    auto_off_time_minutes=DeviceStatusValue(data[12], mapped_val=lambda x: INACTIVE_TIME_MINUTES[x]),
                    wireless_mode=DeviceStatusValue(data[13], 'wireless_mode.speed' if data[13] == 0x00 else 'wireless_mode.range'),
                    wireless_pairing=DeviceStatusValue(data[14], 'pairing.not_paired' if data[14] ==
                                                       0x01 else 'pairing.paired_offline' if data[13] == 0x04 else 'connection.connected'),
                    headset_power_status=DeviceStatusValue(data[15], 'connection.offline' if data[15] ==
                                                           0x01 else 'connection.cable_charging' if data[14] == 0x02 else 'connection.online'),
                )
            else:
                self.log.debug(f'Incoming data from {endpoint.interface}, {endpoint.endpoint}: [{':'.join(hex(x)[2:] for x in data)}]')
        else:
            self.log.debug(f'Incoming data from {endpoint.interface}, {endpoint.endpoint}: [{':'.join(hex(x)[2:] for x in data)}]')

        return DeviceState(
            game_volume=volume,
            chat_volume=volume,
            game_mix=self.game_mix if self.game_mix is not None else 1,
            chat_mix=self.chat_mix if self.chat_mix is not None else 1,
            device_status=device_status,
        )

    @staticmethod
    def packet_0_filler(packet: list[int], size: int):
        return [*packet, *[0 for _ in range(size - len(packet))]]

    def get_configurable_settings(self, state: Optional[DeviceStatus] = None) -> dict[str, list[DeviceSetting]]:
        state = state or DeviceStatus()

        local_settings = self.get_local_settings()

        return {
            'microphone': [
                SliderSetting('mic_volume', 'mic_volume_muted', 'perc_100', 0x01, 0x0a, 1, local_settings['mic_volume'], self.on_mic_volume_change),
                SliderSetting('mic_side_tone', 'mic_side_tone_none', 'mic_side_tone_high', 0x00,
                              0x03, 1, local_settings['mic_side_tone'], self.on_mic_side_tone_change),
                SliderSetting('mic_led_brightness', 'perc_10', 'perc_100', 0x01, 0x0a, 1,
                              local_settings['mic_led_brightness'], self.on_mic_led_brightness_change),
                ToggleSetting('mic_gain', 'mic_gain_low', 'mic_gain_high', local_settings['mic_gain'] == 0x01, self.on_mic_gain_change),
            ],
            'power_management': [
                SliderSetting('pm_shutdown', 'pm_shutdown_disabled', 'pm_shutdown_60_minutes',
                              0x00, 0x06, 1, state.auto_off_time_minutes.value or local_settings['pm_shutdown'], self.on_pm_shutdown_change),
            ],
            'wireless': [
                ToggleSetting('wireless_mode', 'wireless_mode_range', 'wireless_mode_speed', state.wireless_mode.value == 0x01, self.on_wireless_mode_change),
            ],
        }

    def on_mic_volume_change(self, value: int):
        self.send_06_command([0x06, 0x37, value])
        self.send_06_command([0x06, 0x09, 0x00])

        self._local_config['mic_volume'] = value
        self.save_local_settings()

    def on_mic_side_tone_change(self, value: int):
        self.send_06_command([0x06, 0x39, value])
        self.send_06_command([0x06, 0x09, 0x00])

        self._local_config['mic_side_tone'] = value
        self.save_local_settings()

    def on_mic_led_brightness_change(self, value: int):
        self.send_06_command([0x06, 0xbf, value])
        self.send_06_command([0x06, 0x09, 0x00])

        self._local_config['mic_led_brightness'] = value
        self.save_local_settings()

    def on_mic_gain_change(self, value: bool):
        value = 0x02 if value else 0x01

        self.send_06_command([0x06, 0x27, value])
        self.send_06_command([0x06, 0x09, 0x00])

        self._local_config['mic_gain'] = value
        self.save_local_settings()

    def on_pm_shutdown_change(self, value: int):
        self.send_06_command([0x06, 0xc1, value])
        self.send_06_command([0x06, 0x09, 0x00])

        self._local_config['pm_shutdown'] = value
        self.save_local_settings()

        self.send_06_command(STATUS_REQUEST_MESSAGE)

    def on_wireless_mode_change(self, value: bool):
        value = 0x01 if value else 0x00

        self.send_06_command([0x06, 0xc3, value])
        self.send_06_command([0x06, 0x09, 0x00])

        self._local_config['wireless_mode'] = value
        self.save_local_settings()

        self.send_06_command(STATUS_REQUEST_MESSAGE)

    def send_06_command(self, command: list[int], kernel_detach: bool = False) -> None:
        endpoint, _ = self.get_request_device_status()
        commands_endpoint_address = self.device[0].interfaces()[endpoint.interface].endpoints()[endpoint.endpoint].bEndpointAddress
        if kernel_detach:
            self.kernel_detach(endpoint)

        self.device.write(commands_endpoint_address, self.packet_0_filler(command, 64))
