# Use bare String Keys in Layout Resources

Text-bearing Layout Resource positions will continue to treat authored text as one bare String Key, such as `<text>runtimeUi.level</text>`, while code passes the same raw key to the localization API. Radia introduces no reference sigil because these positions already require localized content; complete sentence structure and Inline Content markup belong to the referenced String in `localization.yaml`, and `{argument}` remains reserved for String Placeholders inside that String.
