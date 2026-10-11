package com.example.petshop;

import java.util.ArrayList;
import java.util.List;
import org.springframework.stereotype.Repository;

@Repository
public class OwnerRepository {
    private final List<String> names = new ArrayList<>(List.of("Ada", "Grace"));

    List<String> all() {
        return names;
    }
}
