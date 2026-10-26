use protocol::lludp::{AllocationStrategy, Message, MessageId};
use protocol::schema::{
    BlockMode,
    ids::{
        AGENT_MOVEMENT_COMPLETE, AGENT_THROTTLE, AGENT_UPDATE, AVATAR_APPEARANCE, AVATAR_SIT_RESPONSE, BULK_UPDATE_INVENTORY,
        CHAT_FROM_SIMULATOR, CHAT_FROM_VIEWER, COMPLETE_AGENT_MOVEMENT, COMPLETE_PING_CHECK, CONFIRM_ENABLE_SIMULATOR, DISABLE_SIMULATOR,
        ENABLE_SIMULATOR, GENERIC_MESSAGE, GENERIC_STREAMING_MESSAGE, IMPROVED_TERSE_OBJECT_UPDATE, KILL_OBJECT, LAYER_DATA, LOGOUT_REPLY,
        LOGOUT_REQUEST, MAP_BLOCK_REPLY, MAP_BLOCK_REQUEST, MOVE_INVENTORY_FOLDER, MOVE_INVENTORY_ITEM, OBJECT_EXTRA_PARAMS, OBJECT_UPDATE,
        OBJECT_UPDATE_CACHED, OBJECT_UPDATE_COMPRESSED, PACKET_ACK, PARCEL_PROPERTIES, REGION_HANDLE_REQUEST, REGION_HANDSHAKE,
        REGION_HANDSHAKE_REPLY, REGION_ID_AND_HANDLE_REPLY, REGION_INFO, REMOVE_INVENTORY_FOLDER, REMOVE_INVENTORY_ITEM,
        REMOVE_INVENTORY_OBJECTS, REQUEST_MULTIPLE_OBJECTS, SAVE_ASSET_INTO_INVENTORY, START_PING_CHECK, TELEPORT_CANCEL, TELEPORT_FAILED,
        TELEPORT_FINISH, UPDATE_CREATE_INVENTORY_ITEM, USE_CIRCUIT_CODE,
    },
};
use test_support::lludp::vectors;
pub(super) struct BlockFacts {
    pub(super) name: &'static str,
    pub(super) mode: BlockMode,
    pub(super) fields: &'static str,
}

pub(super) struct TypedMessageCoverage {
    pub(super) name: &'static str,
    pub(super) id: MessageId,
    pub(super) fixture: Option<&'static str>,
    pub(super) preflight: AllocationStrategy,
    pub(super) conformance: ConformanceCategory,
    pub(super) blocks: &'static [BlockFacts],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConformanceCategory {
    SchemaConforming,
    SemanticOverride,
    CompatibilityOverride,
    PackedOverride,
}

macro_rules! typed_message_coverage {
    ($(
        $name:literal => $variant:ident, $id:ident, $fixture:expr,
        $preflight:ident, $conformance:ident, [
            $(($block:literal, $mode:expr, $fields:literal)),* $(,)?
        ];
    )+) => {
        pub(super) const TYPED_MESSAGE_COVERAGE: &[TypedMessageCoverage] = &[
            $(TypedMessageCoverage {
                name: $name,
                id: $id,
                fixture: if $fixture.is_empty() { None } else { Some($fixture) },
                preflight: AllocationStrategy::$preflight,
                conformance: ConformanceCategory::$conformance,
                blocks: &[$(BlockFacts { name: $block, mode: $mode, fields: $fields }),*],
            }),+
        ];

        pub(super) fn typed_variant_name(message: &Message) -> Option<&'static str> {
            match message {
                $(Message::$variant(_) => Some($name),)+
                Message::Schema(_) | Message::Unknown { .. } => None,
            }
        }
    };
}

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
    "RegionInfo" => RegionInfo, REGION_INFO, "",
        SchemaDerived, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
        ("RegionInfo", BlockMode::Single, "SimName EstateID ParentEstateID RegionFlags SimAccess MaxAgents BillableFactor ObjectBonusFactor WaterHeight TerrainRaiseLimit TerrainLowerLimit PricePerMeter RedirectGridX RedirectGridY UseEstateSun SunHour"),
        ("RegionInfo2", BlockMode::Single, "ProductSKU ProductName MaxAgents32 HardMaxAgents HardMaxObjects"),
        ("RegionInfo3", BlockMode::Variable, "RegionFlagsExtended"),
        ("RegionInfo5", BlockMode::Variable, "ChatWhisperRange ChatNormalRange ChatShoutRange ChatWhisperOffset ChatNormalOffset ChatShoutOffset ChatFlags"),
        ("CombatSettings", BlockMode::Variable, "CombatFlags OnDeath DamageThrottle RegenerationRate InvulnerabilyTime DamageLimit"),
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
    "TeleportFinish" => TeleportFinish, TELEPORT_FINISH, "",
        SchemaDerived, SchemaConforming, [
        ("Info", BlockMode::Single, "AgentID LocationID SimIP SimPort RegionHandle SeedCapability SimAccess TeleportFlags"),
    ];
    "TeleportFailed" => TeleportFailed, TELEPORT_FAILED, "",
        SchemaDerived, SchemaConforming, [
        ("Info", BlockMode::Single, "AgentID Reason"),
        ("AlertInfo", BlockMode::Variable, "Message ExtraParams"),
    ];
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
    "ObjectExtraParams" => ObjectExtraParams, OBJECT_EXTRA_PARAMS, vectors::OBJECT_EXTRA_PARAMS,
        Specialized, SchemaConforming, [
            ("AgentData", BlockMode::Single, "AgentID SessionID"),
            ("ObjectData", BlockMode::Variable, "ObjectLocalID ParamType ParamInUse ParamSize ParamData"),
        ];
    "GenericStreamingMessage" => GenericStreamingMessage, GENERIC_STREAMING_MESSAGE, vectors::GENERIC_STREAMING_MESSAGE,
        Specialized, SchemaConforming, [
            ("MethodData", BlockMode::Single, "Method"),
            ("DataBlock", BlockMode::Single, "Data"),
    ];
    "AvatarAppearance" => AvatarAppearance, AVATAR_APPEARANCE, "",
        SchemaDerived, SchemaConforming, [
        ("Sender", BlockMode::Single, "ID IsTrial"),
        ("ObjectData", BlockMode::Single, "TextureEntry"),
        ("VisualParam", BlockMode::Variable, "ParamValue"),
        ("AppearanceData", BlockMode::Variable, "AppearanceVersion CofVersion Flags"),
        ("AppearanceHover", BlockMode::Variable, "HoverHeight"),
        ("AttachmentBlock", BlockMode::Variable, "ID AttachmentPoint"),
    ];
    "AvatarSitResponse" => AvatarSitResponse, AVATAR_SIT_RESPONSE, "",
        SchemaDerived, SchemaConforming, [
        ("SitObject", BlockMode::Single, "ID"),
        ("SitTransform", BlockMode::Single, "AutoPilot SitPosition SitRotation CameraEyeOffset CameraAtOffset ForceMouselook"),
    ];
    "ParcelProperties" => ParcelProperties, PARCEL_PROPERTIES, "",
        SchemaDerived, SchemaConforming, [
        ("ParcelData", BlockMode::Single, "RequestResult SequenceID SnapSelection SelfCount OtherCount PublicCount LocalID OwnerID IsGroupOwned AuctionID ClaimDate ClaimPrice RentPrice AABBMin AABBMax Bitmap Area Status SimWideMaxPrims SimWideTotalPrims MaxPrims TotalPrims OwnerPrims GroupPrims OtherPrims SelectedPrims ParcelPrimBonus OtherCleanTime ParcelFlags SalePrice Name Desc MusicURL MediaURL MediaID MediaAutoScale GroupID PassPrice PassHours Category AuthBuyerID SnapshotID UserLocation UserLookAt LandingType RegionPushOverride RegionDenyAnonymous RegionDenyIdentified RegionDenyTransacted"),
        ("AgeVerificationBlock", BlockMode::Single, "RegionDenyAgeUnverified"),
        ("RegionAllowAccessBlock", BlockMode::Single, "RegionAllowAccessOverride"),
        ("ParcelEnvironmentBlock", BlockMode::Single, "ParcelEnvironmentVersion RegionAllowEnvironmentOverride"),
    ];
    "GenericMessage" => GenericMessage, GENERIC_MESSAGE, "",
        SchemaDerived, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID TransactionID"),
        ("MethodData", BlockMode::Single, "Method Invoice"),
        ("ParamList", BlockMode::Variable, "Parameter"),
    ];
    "LayerData" => LayerData, LAYER_DATA, "",
        SchemaDerived, SchemaConforming, [
        ("LayerID", BlockMode::Single, "Type"),
        ("LayerData", BlockMode::Single, "Data"),
    ];
    "UpdateCreateInventoryItem" => UpdateCreateInventoryItem, UPDATE_CREATE_INVENTORY_ITEM, "",
        SchemaDerived, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SimApproved TransactionID"),
        ("InventoryData", BlockMode::Variable, "ItemID FolderID CallbackID CreatorID OwnerID GroupID BaseMask OwnerMask GroupMask EveryoneMask NextOwnerMask GroupOwned AssetID Type InvType Flags SaleType SalePrice Name Description CreationDate CRC"),
    ];
    "RemoveInventoryItem" => RemoveInventoryItem, REMOVE_INVENTORY_ITEM, "",
        SchemaDerived, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
        ("InventoryData", BlockMode::Variable, "ItemID"),
    ];
    "RemoveInventoryFolder" => RemoveInventoryFolder, REMOVE_INVENTORY_FOLDER, "",
        SchemaDerived, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
        ("FolderData", BlockMode::Variable, "FolderID"),
    ];
    "RemoveInventoryObjects" => RemoveInventoryObjects, REMOVE_INVENTORY_OBJECTS, "",
        SchemaDerived, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID"),
        ("FolderData", BlockMode::Variable, "FolderID"),
        ("ItemData", BlockMode::Variable, "ItemID"),
    ];
    "SaveAssetIntoInventory" => SaveAssetIntoInventory, SAVE_ASSET_INTO_INVENTORY, "",
        NoAlloc, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID"),
        ("InventoryData", BlockMode::Single, "ItemID NewAssetID"),
    ];
    "BulkUpdateInventory" => BulkUpdateInventory, BULK_UPDATE_INVENTORY, "",
        SchemaDerived, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID TransactionID"),
        ("FolderData", BlockMode::Variable, "FolderID ParentID Type Name"),
        ("ItemData", BlockMode::Variable, "ItemID CallbackID FolderID CreatorID OwnerID GroupID BaseMask OwnerMask GroupMask EveryoneMask NextOwnerMask GroupOwned AssetID Type InvType Flags SaleType SalePrice Name Description CreationDate CRC"),
    ];
    "MoveInventoryItem" => MoveInventoryItem, MOVE_INVENTORY_ITEM, "",
        SchemaDerived, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID Stamp"),
        ("InventoryData", BlockMode::Variable, "ItemID FolderID NewName"),
    ];
    "MoveInventoryFolder" => MoveInventoryFolder, MOVE_INVENTORY_FOLDER, "",
        SchemaDerived, SchemaConforming, [
        ("AgentData", BlockMode::Single, "AgentID SessionID Stamp"),
        ("InventoryData", BlockMode::Variable, "FolderID ParentID"),
    ];
}
