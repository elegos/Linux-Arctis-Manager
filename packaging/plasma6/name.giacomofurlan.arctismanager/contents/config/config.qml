import QtQuick

import org.kde.plasma.plasmoid
import org.kde.plasma.configuration

import "../code/i18n.js" as I18n

ConfigModel {
    id: configModel

    // See main.qml's i18nReady for why this is needed: name: below is a
    // direct child binding, evaluated at construction — before
    // Component.onCompleted here ever runs I18n.init() — so without this it
    // gets stuck on whatever translate() returned pre-init.
    property bool i18nReady: false

    Component.onCompleted: {
        var lang = (Qt.locale().name || "en_US").split("_")[0]
        I18n.init(lang)
        configModel.i18nReady = true
    }

    ConfigCategory {
        name: configModel.i18nReady ? I18n.translate("ui", "status") : ""
        icon: "view-list-details"
        source: "ConfigStatusFields.qml"
    }

    ConfigCategory {
        name: configModel.i18nReady ? I18n.translate("ui", "quick_settings") : ""
        icon: "configure"
        source: "ConfigQuickSettings.qml"
    }
}
