import QtQuick

import org.kde.plasma.plasmoid
import org.kde.plasma.configuration

import "../code/i18n.js" as I18n

ConfigModel {
    id: configModel

    Component.onCompleted: I18n.init()

    ConfigCategory {
        name: I18n.translate("ui", "status")
        icon: "view-list-details"
        source: "ConfigStatusFields.qml"
    }

    ConfigCategory {
        name: I18n.translate("ui", "quick_settings")
        icon: "configure"
        source: "ConfigQuickSettings.qml"
    }
}
