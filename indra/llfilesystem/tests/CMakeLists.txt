# -*- cmake -*-

if(BUILD_TESTING)
    add_executable(llfilesystem_test
        lldiriterator_test.cpp
    )
    target_link_libraries(llfilesystem_test
        PRIVATE
        llfilesystem
        GTest::gtest_main
    )
    _CONFIGURE_GTEST_TARGET(llfilesystem_test unit)

    add_executable(llfilesystem_lldir_test
        lldir_test.cpp
    )
    target_link_libraries(llfilesystem_lldir_test
        PRIVATE
        llfilesystem
        GTest::gtest_main
    )
    _CONFIGURE_GTEST_TARGET(llfilesystem_lldir_test integration)
endif()
