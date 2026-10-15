// Inventory fixtures are synthetic and are not independent wire evidence.
typed_message_coverage! {
    "UseCircuitCode" => UseCircuitCode, USE_CIRCUIT_CODE, vectors::USE_CIRCUIT_CODE,
        NoAlloc, SchemaConforming, [
        ("CircuitCode", BlockMode::Single, "Code SessionID ID"),
    ];
    "CompleteAgentMovement" => CompleteAgentMovement, COMPLETE_AGENT_MOVEMENT, vectors::COMPLETE_AGENT_MOVEMENT,
        NoAlloc, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID CircuitCode"),
    ];
    "RegionHandshake" => RegionHandshake, REGION_HANDSHAKE, vectors::REGION_HANDSHAKE,
        Specialized, CompatibilityOverride, [
        ("RegionInfo", BlockMode::Single, "RegionFlags SimAccess SimName SimOwner IsEstateManager WaterHeight BillableFactor CacheID TerrainBase0 TerrainBase1 TerrainBase2 TerrainBase3 TerrainDetail0 TerrainDetail1 TerrainDetail2 TerrainDetail3 TerrainStartHeight00 TerrainStartHeight01 TerrainStartHeight10 TerrainStartHeight11 TerrainHeightRange00 TerrainHeightRange01 TerrainHeightRange10 TerrainHeightRange11"),
        ("RegionInfo2", BlockMode::Single, "RegionID"),
        ("RegionInfo3", BlockMode::Single, "CPUClassID CPURatio ColoName ProductSKU ProductName"),
        ("RegionInfo4", BlockMode::Variable, "RegionFlagsExtended RegionProtocols"),
    ];
    "RegionHandshakeReply" => RegionHandshakeReply, REGION_HANDSHAKE_REPLY, vectors::REGION_HANDSHAKE_REPLY,
        NoAlloc, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
        ("RegionInfo", BlockMode::Single, "Flags"),
    ];
    "AgentMovementComplete" => AgentMovementComplete, AGENT_MOVEMENT_COMPLETE, vectors::AGENT_MOVEMENT_COMPLETE,
        SchemaDerived, SemanticOverride, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
        ("Data", BlockMode::Single, "Position LookAt RegionHandle Timestamp"),
        ("SimData", BlockMode::Single, "ChannelVersion"),
    ];
    "AgentUpdate" => AgentUpdate, AGENT_UPDATE, vectors::AGENT_UPDATE_INITIAL,
        NoAlloc, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID BodyRotation HeadRotation State CameraCenter CameraAtAxis CameraLeftAxis CameraUpAxis Far ControlFlags Flags"),
    ];
    "AgentThrottle" => AgentThrottle, AGENT_THROTTLE, vectors::AGENT_THROTTLE,
        NoAlloc, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID CircuitCode"),
        ("Throttle", BlockMode::Single, "GenCounter Throttles"),
    ];
    "StartPingCheck" => StartPingCheck, START_PING_CHECK, vectors::START_PING_CHECK,
        NoAlloc, SchemaConforming, [
        ("PingID", BlockMode::Single, "PingID OldestUnacked"),
    ];
    "CompletePingCheck" => CompletePingCheck, COMPLETE_PING_CHECK, vectors::COMPLETE_PING_CHECK,
        NoAlloc, SchemaConforming, [
        ("PingID", BlockMode::Single, "PingID"),
    ];
    "PacketAck" => PacketAck, PACKET_ACK, vectors::PACKET_ACK,
        SchemaDerived, SchemaConforming, [
        ("Packets", BlockMode::Variable, "ID"),
    ];
    "TeleportCancel" => TeleportCancel, TELEPORT_CANCEL, vectors::TELEPORT_CANCEL,
        SchemaDerived, SchemaConforming, [("Info", BlockMode::Single, "AgentID SessionID")];
    "LogoutRequest" => LogoutRequest, LOGOUT_REQUEST, vectors::LOGOUT_REQUEST,
        NoAlloc, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
    ];
    "LogoutReply" => LogoutReply, LOGOUT_REPLY, vectors::LOGOUT_REPLY,
        SchemaDerived, CompatibilityOverride, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
        ("InventoryData", BlockMode::Variable, "ItemID"),
    ];
    "ChatFromViewer" => ChatFromViewer, CHAT_FROM_VIEWER, vectors::CHAT_FROM_VIEWER,
        SchemaDerived, SemanticOverride, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
        ("ChatData", BlockMode::Single, "Message Type Channel"),
    ];
    "ChatFromSimulator" => ChatFromSimulator, CHAT_FROM_SIMULATOR, vectors::CHAT_FROM_SIMULATOR,
        SchemaDerived, SemanticOverride, [
        ("ChatData", BlockMode::Single, "FromName SourceID OwnerID SourceType ChatType Audible Position Message"),
    ];
    "RegionHandleRequest" => RegionHandleRequest, REGION_HANDLE_REQUEST, vectors::REGION_HANDLE_REQUEST,
        NoAlloc, SchemaConforming, [
        ("RequestBlock", BlockMode::Single, "RegionID"),
    ];
    "RegionIDAndHandleReply" => RegionIdAndHandleReply, REGION_ID_AND_HANDLE_REPLY, vectors::REGION_ID_AND_HANDLE_REPLY,
        NoAlloc, SchemaConforming, [
        ("ReplyBlock", BlockMode::Single, "RegionID RegionHandle"),
    ];
    "MapBlockRequest" => MapBlockRequest, MAP_BLOCK_REQUEST, vectors::MAP_BLOCK_REQUEST,
        NoAlloc, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID Flags EstateID Godlike"),
        ("PositionData", BlockMode::Single, "MinX MaxX MinY MaxY"),
    ];
    "MapBlockReply" => MapBlockReply, MAP_BLOCK_REPLY, vectors::MAP_BLOCK_REPLY,
        SchemaDerived, SemanticOverride, [
        ("AgentData", BlockMode::Single, "AgentID Flags"),
        ("Data", BlockMode::Variable, "X Y Name Access RegionFlags WaterHeight Agents MapImageID"),
    ];
    "EnableSimulator" => EnableSimulator, ENABLE_SIMULATOR, vectors::ENABLE_SIMULATOR,
        NoAlloc, SchemaConforming, [
        ("SimulatorInfo", BlockMode::Single, "Handle IP Port"),
    ];
    "DisableSimulator" => DisableSimulator, DISABLE_SIMULATOR, vectors::DISABLE_SIMULATOR,
        NoAlloc, SchemaConforming, [];
    "ConfirmEnableSimulator" => ConfirmEnableSimulator, CONFIRM_ENABLE_SIMULATOR, vectors::CONFIRM_ENABLE_SIMULATOR,
        NoAlloc, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
    ];
    "ObjectUpdate" => ObjectUpdate, OBJECT_UPDATE, vectors::OBJECT_UPDATE,
        Specialized, PackedOverride, [
        ("RegionData", BlockMode::Single, "RegionHandle TimeDilation"),
        ("ObjectData", BlockMode::Variable, "ID State FullID CRC PCode Material ClickAction Scale ObjectData ParentID UpdateFlags PathCurve ProfileCurve PathBegin PathEnd PathScaleX PathScaleY PathShearX PathShearY PathTwist PathTwistBegin PathRadiusOffset PathTaperX PathTaperY PathRevolutions PathSkew ProfileBegin ProfileEnd ProfileHollow TextureEntry TextureAnim NameValue Data Text TextColor MediaURL PSBlock ExtraParams Sound OwnerID Gain Flags Radius JointType JointPivot JointAxisOrAnchor"),
    ];
    "ObjectUpdateCompressed" => ObjectUpdateCompressed, OBJECT_UPDATE_COMPRESSED, vectors::OBJECT_UPDATE_COMPRESSED_TYPED,
        Specialized, PackedOverride, [
        ("RegionData", BlockMode::Single, "RegionHandle TimeDilation"),
        ("ObjectData", BlockMode::Variable, "UpdateFlags Data"),
    ];
    "ImprovedTerseObjectUpdate" => ImprovedTerseObjectUpdate, IMPROVED_TERSE_OBJECT_UPDATE, vectors::IMPROVED_TERSE_OBJECT_UPDATE,
        Specialized, PackedOverride, [
        ("RegionData", BlockMode::Single, "RegionHandle TimeDilation"),
        ("ObjectData", BlockMode::Variable, "Data TextureEntry"),
    ];
    "ObjectUpdateCached" => ObjectUpdateCached, OBJECT_UPDATE_CACHED, vectors::OBJECT_UPDATE_CACHED,
        SchemaDerived, SchemaConforming, [
        ("RegionData", BlockMode::Single, "RegionHandle TimeDilation"),
        ("ObjectData", BlockMode::Variable, "ID CRC UpdateFlags"),
    ];
    "KillObject" => KillObject, KILL_OBJECT, vectors::KILL_OBJECT,
        SchemaDerived, SchemaConforming, [
        ("ObjectData", BlockMode::Variable, "ID"),
    ];
    "RequestMultipleObjects" => RequestMultipleObjects, REQUEST_MULTIPLE_OBJECTS, vectors::REQUEST_MULTIPLE_OBJECTS,
        SchemaDerived, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
        ("ObjectData", BlockMode::Variable, "CacheMissType ID"),
    ];
}
